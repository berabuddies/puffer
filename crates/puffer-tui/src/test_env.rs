use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn puffer_home_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) struct ScopedPufferHome {
    old_home: Option<OsString>,
    _guard: MutexGuard<'static, ()>,
}

impl ScopedPufferHome {
    pub(crate) fn new(label: &str) -> Self {
        let guard = puffer_home_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_home = std::env::var_os("PUFFER_HOME");
        let path = stable_home_path(label);
        std::fs::create_dir_all(&path).expect("test PUFFER_HOME");
        std::env::set_var("PUFFER_HOME", path);
        Self {
            old_home,
            _guard: guard,
        }
    }
}

impl Drop for ScopedPufferHome {
    fn drop(&mut self) {
        if let Some(value) = self.old_home.take() {
            std::env::set_var("PUFFER_HOME", value);
        } else {
            std::env::remove_var("PUFFER_HOME");
        }
    }
}

fn stable_home_path(label: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sanitized = label
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    std::env::temp_dir()
        .join("puffer-tui-test-home")
        .join(format!("{}-{now}-{sanitized}", std::process::id()))
}
