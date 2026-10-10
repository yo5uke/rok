//! Progress of downloads.
//!
//! The engine reports what it downloads; the front end decides how to show it (bars on a
//! terminal, one line for R, or nothing). Downloads run in parallel, so the methods are called
//! from several threads at once.

/// Receives the progress of a group of downloads. Every method does nothing by default.
pub trait Transfers: Sync {
    /// A group of `count` downloads begins. `what` names it: "12 packages", "R 4.6.1 (portable)".
    fn begin(&self, _what: &str, _count: usize) {}
    /// A download of `name` starts, or starts again after a retry. `size` is the length the
    /// server announced.
    fn start(&self, _name: &str, _size: Option<u64>) {}
    /// `bytes` more bytes of `name` arrived.
    fn advance(&self, _name: &str, _bytes: u64) {}
    /// `name` is complete.
    fn finish(&self, _name: &str) {}
    /// The group is over: `completed` is false when a download failed.
    fn end(&self, _completed: bool) {}
}

/// Shows nothing.
pub struct Quiet;

impl Transfers for Quiet {}

/// A group of downloads in progress. Dropping it before [`Group::complete`] ends the group as
/// failed, so the front end can clear its display before the error is shown.
pub struct Group<'a> {
    transfers: &'a dyn Transfers,
    completed: bool,
}

impl<'a> Group<'a> {
    pub fn begin(transfers: &'a dyn Transfers, what: &str, count: usize) -> Group<'a> {
        transfers.begin(what, count);
        Group {
            transfers,
            completed: false,
        }
    }

    pub fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for Group<'_> {
    fn drop(&mut self) {
        self.transfers.end(self.completed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);

    impl Transfers for Log {
        fn begin(&self, what: &str, count: usize) {
            self.0.lock().unwrap().push(format!("begin {what} {count}"));
        }
        fn end(&self, completed: bool) {
            self.0.lock().unwrap().push(format!("end {completed}"));
        }
    }

    #[test]
    fn a_group_ends_once_and_says_whether_it_completed() {
        let log = Log::default();
        Group::begin(&log, "2 packages", 2).complete();
        {
            let _failed = Group::begin(&log, "1 package", 1);
        }
        assert_eq!(
            *log.0.lock().unwrap(),
            [
                "begin 2 packages 2",
                "end true",
                "begin 1 package 1",
                "end false"
            ]
        );
    }
}
