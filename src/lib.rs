use std::{
    collections::VecDeque,
    error,
    fmt::Display,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
};

/// Sender half can only be owned by one thread, but it can be cloned
/// to send to other threads.
pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        // increase senders first
        self.inner.senders.fetch_add(1, Ordering::AcqRel);
        Self {
            // clone Arc explicitly
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.inner.senders.fetch_sub(1, Ordering::AcqRel);
        let is_last = self.inner.get_senders() == 0;
        if is_last {
            self.inner.available.notify_one();
        }
    }
}

impl<T> Sender<T> {
    /// Attempts to send a value on this channel
    pub fn send(&mut self, val: T) {
        {
            let mut queue = self.inner.queue.lock().unwrap();
            queue.push_back(val);
        }

        self.inner.available.notify_one();
    }
}

/// Receiver half can only be owned by one thread.
pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
    local: VecDeque<T>,
}

#[derive(PartialEq, Debug)]
pub struct RecvError;

impl Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("receiving error")
    }
}

impl error::Error for RecvError {
    fn description(&self) -> &str {
        "receiving error"
    }
}

impl<T> Receiver<T> {
    /// Attempts to wait for a value on this receiver, returning an error if
    /// no senders available
    pub fn recv(&mut self) -> Result<T, RecvError> {
        if let Some(t) = self.local.pop_front() {
            return Ok(t);
        }
        let mut queue = self.inner.queue.lock().unwrap();
        loop {
            match queue.pop_front() {
                Some(t) => {
                    // steal all rest elements
                    std::mem::swap(&mut self.local, &mut queue);
                    return Ok(t);
                }
                None if self.inner.get_senders() == 0 => return Err(RecvError),
                None => queue = self.inner.available.wait(queue).map_err(|_| RecvError)?,
            }
        }
    }
}

impl<T> Iterator for Receiver<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv().ok()
    }
}

struct Inner<T> {
    queue: Mutex<VecDeque<T>>,
    available: Condvar,
    senders: AtomicUsize,
}

impl<T> Inner<T> {
    fn get_senders(&self) -> usize {
        self.senders.load(Ordering::SeqCst)
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Inner {
        queue: Mutex::new(VecDeque::new()),
        available: Condvar::new(),
        senders: AtomicUsize::new(1),
    };
    let inner = Arc::new(inner);
    (
        Sender {
            inner: inner.clone(),
        },
        Receiver {
            inner: inner.clone(),
            local: VecDeque::new(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_case_should_works() {
        let (mut tx, mut rx) = channel();
        tx.send(1);
        tx.send(234);
        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(rx.recv(), Ok(234));
    }

    #[test]
    fn iterator_should_works() {
        let (mut tx1, rx) = channel();
        tx1.send(1);

        let mut tx2 = tx1.clone();
        tx2.send(234);

        drop(tx1);
        drop(tx2);

        let resp: Vec<i32> = rx.collect();
        assert_eq!(vec![1, 234], resp);
    }

    #[test]
    fn should_not_blocking() {
        let (tx, mut rx) = channel::<()>();
        drop(tx);

        assert!(rx.recv().is_err());
    }
}
