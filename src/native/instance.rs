//! Keeps a second launch from opening the same sessions while the first is in the tray.

#[cfg(windows)]
pub(super) struct Instance(windows_sys::Win32::Foundation::HANDLE);

#[cfg(not(windows))]
pub(super) struct Instance(std::fs::File);

impl Instance {
    pub fn acquire() -> std::io::Result<Option<Self>> {
        #[cfg(windows)]
        {
            let mut name = if crate::demo() {
                r"Local\TracePlotter-demo"
            } else {
                r"Local\TracePlotter"
            }
            .to_owned();
            if let Some(id) = crate::instance_id() {
                name.push('-');
                name.push_str(&id.to_string());
            }
            Self::named(&name)
        }
        #[cfg(not(windows))]
        {
            std::fs::create_dir_all(crate::data_dir())?;
            Self::at(&crate::data_dir().join("instance.lock"))
        }
    }

    #[cfg(windows)]
    fn named(name: &str) -> std::io::Result<Option<Self>> {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // Safety: the name is null-terminated and lives through the call. The returned
        // handle is kept until shutdown, including while the window is hidden.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // Safety: GetLastError immediately follows the successful CreateMutexW call.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // Safety: this launch owns the returned handle, but is not keeping it.
            unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }

    #[cfg(not(windows))]
    fn at(path: &std::path::Path) -> std::io::Result<Option<Self>> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self(file))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

#[cfg(windows)]
impl Drop for Instance {
    fn drop(&mut self) {
        // Safety: this guard owns the handle and closes it exactly once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(not(windows))]
impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_launches_are_rejected_until_the_first_instance_exits() {
        #[cfg(windows)]
        let name = format!(r"Local\TracePlotter-test-{}", uuid::Uuid::new_v4());
        #[cfg(windows)]
        let acquire = || Instance::named(&name);
        #[cfg(not(windows))]
        let dir = tempfile::tempdir().unwrap();
        #[cfg(not(windows))]
        let acquire = || Instance::at(&dir.path().join("instance.lock"));

        let first = acquire().unwrap().unwrap();
        assert!(acquire().unwrap().is_none());
        assert!(acquire().unwrap().is_none());
        drop(first);
        assert!(acquire().unwrap().is_some());
    }
}
