use std::collections::HashMap;
use std::io::{self, Read, Write as IoWrite};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_PROCESSES: usize = 64;
const LRU_PROTECTED: usize = 8;
const HEAD_CAPACITY: usize = 512 * 1024;
const TAIL_CAPACITY: usize = 512 * 1024;

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub raw: Vec<u8>,
    pub process_id: Option<i32>,
    pub exit_code: Option<i32>,
}

pub(crate) struct ProcessEntry {
    pub process_id: i32,
    pub command: String,
    pub tty: bool,
    output_buffer: Arc<Mutex<HeadTailBuffer>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    last_used: Instant,
    #[cfg(unix)]
    pty_master_fd: Option<i32>,
    pipe_stdin: Option<std::process::ChildStdin>,
}

impl ProcessEntry {
    pub fn write_stdin(&mut self, data: &[u8]) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(master_fd) = self.pty_master_fd {
            let written = unsafe {
                libc::write(
                    master_fd,
                    data.as_ptr() as *const libc::c_void,
                    data.len(),
                )
            };
            if written < 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }

        if let Some(ref mut stdin) = self.pipe_stdin {
            stdin.write_all(data)?;
            stdin.flush()?;
        }
        Ok(())
    }

    pub fn collect_output(&self) -> Vec<u8> {
        let buffer = self.output_buffer.lock().unwrap();
        buffer.snapshot()
    }

    pub fn collect_output_since(&self, after_bytes: usize) -> Vec<u8> {
        let buffer = self.output_buffer.lock().unwrap();
        buffer.since(after_bytes)
    }

    pub fn total_output_bytes(&self) -> usize {
        self.output_buffer.lock().unwrap().total_written()
    }

    pub fn exit_code(&self) -> Option<i32> {
        *self.exit_code.lock().unwrap()
    }

    pub fn has_exited(&self) -> bool {
        self.exit_code().is_some()
    }

    pub fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(fd) = self.pty_master_fd.take() {
            unsafe { libc::close(fd) };
        }
        self.pipe_stdin.take();
    }
}

impl Drop for ProcessEntry {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(fd) = self.pty_master_fd.take() {
            unsafe { libc::close(fd) };
        }
    }
}

pub struct ProcessStore {
    processes: HashMap<i32, ProcessEntry>,
    next_id: i32,
}

impl std::fmt::Debug for ProcessStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessStore")
            .field("count", &self.processes.len())
            .field("next_id", &self.next_id)
            .finish()
    }
}

impl Default for ProcessStore {
    fn default() -> Self {
        Self {
            processes: HashMap::new(),
            next_id: 1000,
        }
    }
}

impl ProcessStore {
    pub fn allocate_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn insert(&mut self, entry: ProcessEntry) {
        if self.processes.len() >= MAX_PROCESSES {
            self.evict_one();
        }
        self.processes.insert(entry.process_id, entry);
    }

    pub fn get_mut(&mut self, process_id: i32) -> Option<&mut ProcessEntry> {
        if let Some(entry) = self.processes.get_mut(&process_id) {
            entry.last_used = Instant::now();
            Some(entry)
        } else {
            None
        }
    }

    pub fn peek(&self, process_id: i32) -> Option<&ProcessEntry> {
        self.processes.get(&process_id)
    }

    pub fn remove(&mut self, process_id: i32) -> Option<ProcessEntry> {
        self.processes.remove(&process_id)
    }

    pub fn drain_exited(&mut self) -> Vec<(i32, String, Option<i32>)> {
        let exited: Vec<i32> = self
            .processes
            .iter()
            .filter(|(_, e)| e.has_exited())
            .map(|(&id, _)| id)
            .collect();
        exited
            .into_iter()
            .filter_map(|id| {
                let entry = self.processes.remove(&id)?;
                Some((id, entry.command.clone(), entry.exit_code()))
            })
            .collect()
    }

    pub fn terminate_all(&mut self) {
        for (_, entry) in self.processes.iter_mut() {
            entry.terminate();
        }
        self.processes.clear();
    }

    fn evict_one(&mut self) {
        let mut candidates: Vec<(i32, bool, Instant)> = self
            .processes
            .iter()
            .map(|(&id, entry)| (id, entry.has_exited(), entry.last_used))
            .collect();
        candidates.sort_by(|a, b| a.2.cmp(&b.2));

        if candidates.len() <= LRU_PROTECTED {
            return;
        }

        let evictable = &candidates[..candidates.len() - LRU_PROTECTED];
        let victim = evictable
            .iter()
            .find(|(_, exited, _)| *exited)
            .or_else(|| evictable.first());

        if let Some(&(id, _, _)) = victim {
            if let Some(mut entry) = self.processes.remove(&id) {
                entry.terminate();
            }
        }
    }
}

struct HeadTailBuffer {
    head: Vec<u8>,
    tail: Vec<u8>,
    total_written: usize,
}

impl Default for HeadTailBuffer {
    fn default() -> Self {
        Self {
            head: Vec::with_capacity(HEAD_CAPACITY),
            tail: Vec::with_capacity(TAIL_CAPACITY),
            total_written: 0,
        }
    }
}

impl HeadTailBuffer {
    fn push(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.total_written += data.len();

        let head_remaining = HEAD_CAPACITY.saturating_sub(self.head.len());
        if head_remaining > 0 {
            let take = data.len().min(head_remaining);
            self.head.extend_from_slice(&data[..take]);
            if take < data.len() {
                self.push_tail(&data[take..]);
            }
        } else {
            self.push_tail(data);
        }
    }

    fn push_tail(&mut self, data: &[u8]) {
        if data.len() >= TAIL_CAPACITY {
            self.tail.clear();
            self.tail
                .extend_from_slice(&data[data.len() - TAIL_CAPACITY..]);
        } else if self.tail.len() + data.len() > TAIL_CAPACITY {
            let drain = (self.tail.len() + data.len()) - TAIL_CAPACITY;
            self.tail.drain(..drain);
            self.tail.extend_from_slice(data);
        } else {
            self.tail.extend_from_slice(data);
        }
    }

    fn snapshot(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.head.len() + self.tail.len());
        out.extend_from_slice(&self.head);
        out.extend_from_slice(&self.tail);
        out
    }

    fn since(&self, after_bytes: usize) -> Vec<u8> {
        if after_bytes >= self.total_written {
            return Vec::new();
        }
        let full = self.snapshot();
        let available = self.total_written.min(full.len());
        let skip = after_bytes.saturating_sub(self.total_written - available);
        full[skip..].to_vec()
    }

    fn total_written(&self) -> usize {
        self.total_written
    }
}

#[cfg(unix)]
pub(crate) fn spawn_pty_process(
    command: &str,
    cwd: &std::path::Path,
    process_id: i32,
    store_buffer: Arc<Mutex<HeadTailBuffer>>,
    store_exit: Arc<Mutex<Option<i32>>>,
) -> io::Result<ProcessEntry> {
    use std::os::unix::process::CommandExt;

    let mut master_fd: libc::c_int = 0;
    let mut slave_fd: libc::c_int = 0;
    let mut ws = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let ret = unsafe { libc::openpty(&mut master_fd, &mut slave_fd, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    let shell = puffer_tools::detected_shell();
    let mut cmd = std::process::Command::new(shell);
    cmd.arg("-lc").arg(command).current_dir(cwd);

    // Each Stdio::from_raw_fd takes ownership, so dup the slave for stdout/stderr.
    let slave_stdout = unsafe { libc::dup(slave_fd) };
    let slave_stderr = unsafe { libc::dup(slave_fd) };
    if slave_stdout < 0 || slave_stderr < 0 {
        unsafe {
            libc::close(slave_fd);
            if slave_stdout >= 0 { libc::close(slave_stdout); }
            if slave_stderr >= 0 { libc::close(slave_stderr); }
            libc::close(master_fd);
        }
        return Err(io::Error::last_os_error());
    }

    unsafe {
        cmd.stdin(Stdio::from_raw_fd(slave_fd));
        cmd.stdout(Stdio::from_raw_fd(slave_stdout));
        cmd.stderr(Stdio::from_raw_fd(slave_stderr));
    }

    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0);
            Ok(())
        });
    }

    let child = cmd.spawn()?;

    let reader_buffer = Arc::clone(&store_buffer);
    let reader_master = master_fd;

    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let n = unsafe {
                libc::read(
                    reader_master,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if n <= 0 {
                break;
            }
            let mut buffer = reader_buffer.lock().unwrap();
            buffer.push(&buf[..n as usize]);
        }
    });

    let waiter_exit = Arc::clone(&store_exit);
    let mut waiter_child = child;

    std::thread::spawn(move || {
        let status = waiter_child.wait();
        let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
        *waiter_exit.lock().unwrap() = Some(code);
    });

    Ok(ProcessEntry {
        process_id,
        command: command.to_string(),
        tty: true,
        output_buffer: store_buffer,
        exit_code: store_exit,
        last_used: Instant::now(),
        pty_master_fd: Some(master_fd),
        pipe_stdin: None,
    })
}

#[cfg(unix)]
use std::os::unix::io::FromRawFd;

pub(crate) fn spawn_pipe_process(
    command: &str,
    cwd: &std::path::Path,
    process_id: i32,
    store_buffer: Arc<Mutex<HeadTailBuffer>>,
    store_exit: Arc<Mutex<Option<i32>>>,
) -> io::Result<ProcessEntry> {
    let shell = puffer_tools::detected_shell();
    let mut child = std::process::Command::new(shell)
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_buffer = Arc::clone(&store_buffer);
    if let Some(mut stdout) = stdout {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        stdout_buffer.lock().unwrap().push(&buf[..n]);
                    }
                }
            }
        });
    }

    let stderr_buffer = Arc::clone(&store_buffer);
    if let Some(mut stderr) = stderr {
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        stderr_buffer.lock().unwrap().push(&buf[..n]);
                    }
                }
            }
        });
    }

    let waiter_exit = Arc::clone(&store_exit);

    // The child handle must move into the waiter thread so it can call wait().
    // We keep stdin alive in the entry for write_stdin support.
    let stdin = child.stdin.take();
    let child_id = child.id();

    std::thread::spawn(move || {
        let status = child.wait();
        let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
        *waiter_exit.lock().unwrap() = Some(code);
    });

    // Build a synthetic entry — the child handle is consumed by the waiter,
    // but we keep stdin for interactive writes.
    Ok(ProcessEntry {
        process_id,
        command: command.to_string(),
        tty: false,
        output_buffer: store_buffer,
        exit_code: store_exit,
        last_used: Instant::now(),
        #[cfg(unix)]
        pty_master_fd: None,
        pipe_stdin: stdin,
    })
}

pub(crate) fn spawn_tracked_process(
    command: &str,
    cwd: &std::path::Path,
    process_id: i32,
    tty: bool,
) -> io::Result<ProcessEntry> {
    let buffer = Arc::new(Mutex::new(HeadTailBuffer::default()));
    let exit_code = Arc::new(Mutex::new(None));

    #[cfg(unix)]
    if tty {
        return spawn_pty_process(command, cwd, process_id, buffer, exit_code);
    }

    spawn_pipe_process(command, cwd, process_id, buffer, exit_code)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_tail_buffer_small_data() {
        let mut buf = HeadTailBuffer::default();
        buf.push(b"hello");
        assert_eq!(buf.snapshot(), b"hello");
        assert_eq!(buf.total_written(), 5);
    }

    #[test]
    fn head_tail_buffer_overflow_to_tail() {
        let mut buf = HeadTailBuffer::default();
        let head_data = vec![b'A'; HEAD_CAPACITY];
        buf.push(&head_data);
        assert_eq!(buf.head.len(), HEAD_CAPACITY);
        assert!(buf.tail.is_empty());

        buf.push(b"tail_data");
        assert_eq!(buf.tail, b"tail_data");
    }

    #[test]
    fn head_tail_buffer_tail_rotation() {
        let mut buf = HeadTailBuffer::default();
        buf.push(&vec![b'A'; HEAD_CAPACITY]);
        buf.push(&vec![b'B'; TAIL_CAPACITY]);
        assert_eq!(buf.tail.len(), TAIL_CAPACITY);

        buf.push(b"overflow");
        assert_eq!(buf.tail.len(), TAIL_CAPACITY);
        assert!(buf.tail.ends_with(b"overflow"));
    }

    #[test]
    fn head_tail_buffer_since() {
        let mut buf = HeadTailBuffer::default();
        buf.push(b"hello world");
        let since = buf.since(5);
        assert_eq!(since, b" world");
    }

    #[test]
    fn process_store_allocate_sequential() {
        let mut store = ProcessStore::default();
        assert_eq!(store.allocate_id(), 1000);
        assert_eq!(store.allocate_id(), 1001);
        assert_eq!(store.allocate_id(), 1002);
    }

    #[test]
    fn process_store_drain_exited() {
        let mut store = ProcessStore::default();
        let buffer = Arc::new(Mutex::new(HeadTailBuffer::default()));
        let exit_code = Arc::new(Mutex::new(Some(0)));
        store.insert(ProcessEntry {
            process_id: 1000,
            command: "echo done".to_string(),
            tty: false,
            output_buffer: buffer,
            exit_code,
            last_used: Instant::now(),
            #[cfg(unix)]
            pty_master_fd: None,
            pipe_stdin: None,
        });

        let exited = store.drain_exited();
        assert_eq!(exited.len(), 1);
        assert_eq!(exited[0].0, 1000);
        assert_eq!(exited[0].2, Some(0));
        assert!(store.processes.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn spawn_pty_process_runs_command() {
        let temp = tempfile::tempdir().unwrap();
        let buffer = Arc::new(Mutex::new(HeadTailBuffer::default()));
        let exit_code = Arc::new(Mutex::new(None));

        let entry = spawn_pty_process(
            "printf 'pty-test-output'",
            temp.path(),
            9999,
            buffer,
            exit_code,
        )
        .unwrap();

        assert_eq!(entry.process_id, 9999);
        assert!(entry.tty);

        // Wait for process to finish and output to be collected
        std::thread::sleep(std::time::Duration::from_millis(500));

        let output = entry.collect_output();
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("pty-test-output"),
            "expected 'pty-test-output' in: {text}"
        );
    }

    #[test]
    fn spawn_pipe_process_runs_command() {
        let temp = tempfile::tempdir().unwrap();
        let buffer = Arc::new(Mutex::new(HeadTailBuffer::default()));
        let exit_code = Arc::new(Mutex::new(None));

        let mut entry = spawn_pipe_process(
            "printf 'pipe-test'",
            temp.path(),
            8888,
            buffer,
            exit_code,
        )
        .unwrap();

        assert_eq!(entry.process_id, 8888);
        assert!(!entry.tty);

        // Wait for process to exit and output to be collected
        std::thread::sleep(std::time::Duration::from_millis(500));

        let output = entry.collect_output();
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("pipe-test"),
            "expected 'pipe-test' in: {text}"
        );
    }
}
