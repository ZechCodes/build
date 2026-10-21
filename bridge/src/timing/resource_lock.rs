//! Keep diagnostic acquisition accounting out of ordinary lock expressions.
macro_rules! resource_lock {
    ($mutex:expr) => {{
        #[cfg(test)]
        {
            $crate::timing::write_wait::mutex::acquire_mutex(
                &$mutex,
                $crate::timing::write_wait::WaitKind::ResourceLock,
            )
        }
        #[cfg(not(test))]
        {
            ($mutex).lock()
        }
    }};
}

pub(crate) use resource_lock;
