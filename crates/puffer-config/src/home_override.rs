use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    static PUFFER_HOME_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Restores the previous thread-local Puffer home override when dropped.
#[derive(Debug)]
pub struct PufferHomeOverride {
    previous: Option<PathBuf>,
}

impl Drop for PufferHomeOverride {
    fn drop(&mut self) {
        PUFFER_HOME_OVERRIDE.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

/// Overrides the Puffer home path for `ConfigPaths::discover` on the current thread.
pub fn set_puffer_home_override(path: impl Into<PathBuf>) -> PufferHomeOverride {
    PUFFER_HOME_OVERRIDE.with(|slot| PufferHomeOverride {
        previous: slot.replace(Some(path.into())),
    })
}

/// Returns the current thread-local Puffer home override, if one is active.
pub(crate) fn puffer_home_override() -> Option<PathBuf> {
    PUFFER_HOME_OVERRIDE.with(|slot| slot.borrow().clone())
}
