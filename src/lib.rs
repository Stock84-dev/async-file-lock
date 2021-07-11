#![deny(unused_must_use)]
// #![cfg_attr(test, feature(test))]

use std::task::{Context, Poll};
use std::pin::Pin;
use std::fmt::Formatter;
use std::fmt::Debug;
use std::future::Future;
use std::io::{Error, Result, SeekFrom, Seek};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite};
use tokio::task::{spawn_blocking, JoinHandle};
use futures_lite::{ready, FutureExt};
use fs3::FileExt;
use std::path::Path;
use std::mem::MaybeUninit;

/// Locks a file asynchronously.
/// Auto locks a file if any read or write methods are called. If [Self::lock_exclusive]
/// or [Self::lock_shared] has been called then the file will stay locked.
/// Can auto seek to specified location before doing any read/write operation.
///
/// Note 1: Do not attempt to have multiple file handles for the same file. Because locking is done
/// per process basis.
/// Note 2: Remember to open a file with specified read and/or write mode as write calls will just
/// be ignored if the file is opened in read mode.
pub struct FileLock {
    mode: SeekFrom,
    state: State,
    is_manually_locked: bool,
    unlocked_file: Option<std::fs::File>,
    locked_file: Option<File>,
    result: Option<Result<u64>>,
    locking_fut: Option<JoinHandle<std::result::Result<File, (std::fs::File, Error)>>>,
    unlocking_fut: Option<Pin<Box<dyn Future<Output = std::fs::File> + Send>>>,
    seek_fut: Option<JoinHandle<(Result<u64>, std::fs::File)>>,
}

impl FileLock {
    /// Opens a file in read and write mode that is unlocked.
    // This function will create a file if it does not exist, and will truncate it if it does.
    pub async fn create(path: impl AsRef<Path>) -> Result<FileLock> {
        let file = OpenOptions::new().write(true).read(true).create(true).truncate(true).open(path).await?;
        Ok(FileLock::new_tokio(file).await)
    }

    /// Attempts to open a file in read and write mode that is unlocked.
    pub async fn open(path: impl AsRef<Path>) -> Result<FileLock> {
        let file = OpenOptions::new().write(true).read(true).open(path).await?;
        Ok(FileLock::new_tokio(file).await)
    }

    /// Creates a new 'FileLock' from [`tokio::fs::File`].
    pub async fn new_tokio(tokio_file: File) -> FileLock {
        FileLock {
            mode: SeekFrom::Current(0),
            state: State::Unlocked,
            is_manually_locked: false,
            unlocked_file: Some(tokio_file.into_std().await),
            locked_file: None,
            result: None,
            locking_fut: None,
            unlocking_fut: None,
            seek_fut: None
        }
    }

    /// Creates a new 'FileLock' from [`std::fs::File`].
    pub fn new_std(std_file: std::fs::File) -> FileLock {
        FileLock {
            mode: SeekFrom::Current(0),
            state: State::Unlocked,
            is_manually_locked: false,
            unlocked_file: Some(std_file),
            locked_file: None,
            result: None,
            locking_fut: None,
            unlocking_fut: None,
            seek_fut: None
        }
    }

    /// Locks the file for reading and writing until [`Self::unlock`] is called.
    pub fn lock_exclusive(&mut self) -> LockFuture {
        if self.locked_file.is_some() {
            panic!("File already locked.");
        }
        self.is_manually_locked = true;
        LockFuture::new_exclusive(self)
    }

    /// Locks the file for reading and writing until [`Self::unlock`] is called. Returns an error if
    /// the file is currently locked.
    pub fn try_lock_exclusive(&mut self) -> Result<()> {
        if self.locked_file.is_some() {
            panic!("File already locked.");
        }
        self.is_manually_locked = true;
        self.unlocked_file.as_mut().unwrap().try_lock_exclusive().map(|_| {
            self.locked_file = Some(File::from_std(self.unlocked_file.take().unwrap()));
            self.state = State::Locked;

        })
    }

    /// Locks the file for reading until [`Self::unlock`] is called.
    pub fn lock_shared(&mut self) -> LockFuture {
        if self.locked_file.is_some() {
            panic!("File already locked.");
        }
        self.is_manually_locked = true;
        LockFuture::new_shared(self)
    }

    /// Locks the file for reading until [`Self::unlock`] is called. Returns an error if the file
    /// is currently locked.
    pub fn try_lock_shared(&mut self) -> Result<()> {
        if self.locked_file.is_some() {
            panic!("File already locked.");
        }
        self.is_manually_locked = true;
        self.unlocked_file.as_mut().unwrap().try_lock_shared().map(|_| {
            self.locked_file = Some(File::from_std(self.unlocked_file.take().unwrap()));
            self.state = State::Locked;

        })
    }

    /// Unlocks the file.
    pub fn unlock(&mut self) -> UnlockFuture {
        if self.unlocked_file.is_some() {
            panic!("File already unlocked.");
        }
        self.is_manually_locked = false;
        UnlockFuture::new(self)
    }

    /// Sets auto seeking mode. File will always seek to specified location before doing any
    /// read/write operation.
    pub fn set_seeking_mode(&mut self, mode: SeekFrom) {
        self.mode = mode;
    }

    pub fn seeking_mode(&self) -> SeekFrom {
        self.mode
    }

    /// Attempts to sync all OS-internal metadata to disk.
    ///
    /// This function will attempt to ensure that all in-core data reaches the
    /// filesystem before returning.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tokio::fs::File;
    /// use tokio::prelude::*;
    ///
    /// # async fn dox() -> std::io::Result<()> {
    /// let mut file = File::create("foo.txt").await?;
    /// file.write_all(b"hello, world!").await?;
    /// file.sync_all().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The [`write_all`] method is defined on the [`AsyncWriteExt`] trait.
    ///
    /// [`write_all`]: fn@crate::io::AsyncWriteExt::write_all
    /// [`AsyncWriteExt`]: trait@crate::io::AsyncWriteExt
    pub async fn sync_all(&mut self) -> Result<()> {
        if let Some(file) = &mut self.locked_file {
            return file.sync_all().await;
        }
        let file = self.unlocked_file.take().unwrap();
        let (result, file) = spawn_blocking(|| {
            (file.sync_all(), file)
        }).await.unwrap();
        self.unlocked_file = Some(file);
        result
    }

    /// This function is similar to `sync_all`, except that it may not
    /// synchronize file metadata to the filesystem.
    ///
    /// This is intended for use cases that must synchronize content, but don't
    /// need the metadata on disk. The goal of this method is to reduce disk
    /// operations.
    ///
    /// Note that some platforms may simply implement this in terms of `sync_all`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tokio::fs::File;
    /// use tokio::prelude::*;
    ///
    /// # async fn dox() -> std::io::Result<()> {
    /// let mut file = File::create("foo.txt").await?;
    /// file.write_all(b"hello, world!").await?;
    /// file.sync_data().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The [`write_all`] method is defined on the [`AsyncWriteExt`] trait.
    ///
    /// [`write_all`]: fn@crate::io::AsyncWriteExt::write_all
    /// [`AsyncWriteExt`]: trait@crate::io::AsyncWriteExt
    pub async fn sync_data(&mut self) -> Result<()> {
        if let Some(file) = &mut self.locked_file {
            return file.sync_data().await;
        }
        let file = self.unlocked_file.take().unwrap();
        let (result, file) = spawn_blocking(|| {
            (file.sync_data(), file)
        }).await.unwrap();
        self.unlocked_file = Some(file);
        result
    }

    /// Gets a reference to the file.
    ///
    /// If the file is locked it will be in the second element of a tuple as [`tokio::fs::File`]
    /// otherwise it will be in the first element as [`std::fs::File`].
    /// It is inadvisable to directly read/write from/to the file.
    pub fn get_ref(&self) -> (Option<&std::fs::File>, Option<&File>) {
        (self.unlocked_file.as_ref(), self.locked_file.as_ref())
    }

    /// Gets a mutable reference to the file.
    ///
    /// If the file is locked it will be in the second element of a tuple as [`tokio::fs::File`]
    /// otherwise it will be in the first element as [`std::fs::File`].
    /// It is inadvisable to directly read/write from/to the file.
    pub fn get_mut(&mut self) -> (Option<&mut std::fs::File>, Option<&mut File>) {
        (self.unlocked_file.as_mut(), self.locked_file.as_mut())
    }

    fn poll_exclusive_lock(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        loop {
            match &mut self.locking_fut {
                None => {
                    LockFuture::new_exclusive(self);
                }
                Some(_) => return self.poll_locking_fut(cx),
            }
        }
    }

    fn poll_shared_lock(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        loop {
            match &mut self.locking_fut {
                None => {
                    LockFuture::new_shared(self);
                }
                Some(_) => return self.poll_locking_fut(cx),
            }
        }
    }

    fn poll_unlock(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        loop {
            match &mut self.unlocking_fut {
                None => {
                    UnlockFuture::new(self);
                }
                Some(fut) => {
                    // println!("unlocking");
                    let file = ready!(fut.poll(cx));
                    let result = file.unlock();
                    self.unlocked_file = Some(file);
                    if let Err(e) = result {
                        self.result = Some(Err(e));
                    }
                    self.state = State::Unlocked;
                    self.unlocking_fut.take();
                    // println!("unlocked");
                    return Poll::Ready(());
                }
            }
        }
    }

    fn poll_locking_fut(&mut self, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let result = ready!(self.locking_fut.as_mut().unwrap().poll(cx)).unwrap();
        self.locking_fut.take();
        return match result {
            Ok(file) => {
                self.locked_file = Some(file);
                self.state = State::Locked;
                Poll::Ready(Ok(()))
            }
            Err((file, e)) => {
                self.unlocked_file = Some(file);
                self.state = State::Unlocked;
                Poll::Ready(Err(e))
            }
        };
    }
}

macro_rules! poll_loop {
    ($self:ident, $cx:ident, $unlocked_map:expr, $lock:ident, State::Working => $working:block) => {
        loop {
            match $self.state {
                State::Unlocked => {
                    if let Some(result) = $self.result.take() {
                        return Poll::Ready(result.map($unlocked_map));
                    }
                    $self.state = State::Locking;
                }
                State::Unlocking => ready!($self.poll_unlock($cx)),
                #[allow(unused_must_use)]
                State::Locked => match $self.mode {
                    SeekFrom::Current(0) => $self.state = State::Working,
                    _ => {
                        let mode = $self.mode;
                        $self.as_mut().start_seek($cx, mode);
                        $self.state = State::Seeking;
                    }
                },
                State::Working => {
                    // println!("working");
                    $working
                    // println!("worked");
                },
                State::Locking => {
                    if let Err(e) = ready!($self.$lock($cx)) {
                        return Poll::Ready(Err(e));
                    }
                }
                State::Seeking => match ready!($self.as_mut().poll_complete($cx)) {
                    Ok(_) => $self.state = State::Working,
                    Err(e) => return Poll::Ready(Err(e)),
                },
            }
        }
    };
}

impl AsyncWrite for FileLock {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        poll_loop! {self, cx, |x| x as usize, poll_exclusive_lock,
            State::Working => {
                let result = ready!(Pin::new(self.locked_file.as_mut().unwrap())
                        .as_mut()
                        .poll_write(cx, buf));
                // println!("written {:?}", &buf[..*result.as_ref().unwrap()]);
                if self.is_manually_locked {
                    self.state = State::Locked;
                    return Poll::Ready(result);
                } else {
                    self.state = State::Unlocking;
                    self.result = Some(result.map(|x| x as u64));
                }
            }
            // State::Flushing => {
            //     if let Err(e) = ready!(self.as_mut().poll_flush(cx)) {
            //         self.result = Some(Err(e));
            //     }
            //     self.state = State::Unlocking;
            // }
        };
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        // println!("flushing");
        poll_loop! {self, cx, |_| (), poll_exclusive_lock,
            State::Working => {
                let result = ready!(Pin::new(self.locked_file.as_mut().unwrap())
                        .as_mut()
                        .poll_flush(cx));
                // println!("flushed");
                if self.is_manually_locked {
                    self.state = State::Locked;
                    return Poll::Ready(result);
                } else {
                    self.state = State::Unlocking;
                    self.result = Some(result.map(|_| 0));
                }
            }
            // State::Flushing => {
            //     let result = ready!(Pin::new(self.locked_file.as_mut().unwrap())
            //             .as_mut()
            //             .poll_flush(cx));
            //     // println!("flushed");
            //     return Poll::Ready(result);
            // }
        };
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        // println!("shutting down");
        // We don't have to do anything as files are unlocked when underlying tokio file reports
        // some progress. Looking at implementation of shutdown for `tokio::fs::File` says that it
        // does nothing.
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for FileLock {
    unsafe fn prepare_uninitialized_buffer(&self, _: &mut [MaybeUninit<u8>]) -> bool {
        false
    }

    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        poll_loop! {self, cx, |x| x as usize, poll_shared_lock,
            State::Working => {
                let result = ready!(Pin::new(self.locked_file.as_mut().unwrap())
                        .as_mut()
                        .poll_read(cx, buf));
                if self.is_manually_locked {
                    self.state = State::Locked;
                    return Poll::Ready(result);
                } else {
                    self.state = State::Unlocking;
                    self.result = Some(result.map(|x| x as u64));
                }
            }
        };
    }
}

impl AsyncSeek for FileLock {
    fn start_seek(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        position: SeekFrom,
    ) -> Poll<Result<()>> {
        if let Some(ref mut locked_file) = self.locked_file {
            return Pin::new(locked_file)
                .as_mut()
                .start_seek(cx, position);
        }
        let mut file = self.unlocked_file.take().expect("Cannot seek while in the process of locking/unlocking/seeking");
        self.seek_fut = Some(spawn_blocking(move || {
            (file.seek(position), file)
        }));
        return Poll::Ready(Ok(()));
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u64>> {
        if let Some(ref mut locked_file) = self.locked_file {
            return Pin::new(locked_file)
                    .as_mut()
                    .poll_complete(cx)
        }
        let (result, file) = ready!(Pin::new(self.seek_fut.as_mut().unwrap()).poll(cx)).unwrap();
        self.seek_fut = None;
        self.unlocked_file = Some(file);
        return Poll::Ready(result);
    }
}

impl Debug for FileLock {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("FileLock");
        match self.state {
            State::Unlocked => {
                debug.field("file", self.unlocked_file.as_ref().unwrap());
            }
            State::Locked => {
                debug.field("file", self.locked_file.as_ref().unwrap());
            }
            _ => panic!("Invalid state"),
        }
        debug.field("mode", &self.mode).finish()
    }
}

enum State {
    Unlocked,
    Unlocking,
    Locked,
    Locking,
    Seeking,
    Working,
}

pub struct LockFuture<'a> {
    file_lock: &'a mut FileLock,
}

impl<'a> LockFuture<'a> {
    fn new_exclusive(file_lock: &'a mut FileLock) -> LockFuture<'a> {
        // println!("locking exclusive");
        let unlocked_file = file_lock.unlocked_file.take().unwrap();
        file_lock.locking_fut = Some(spawn_blocking(move || {
            let result = match unlocked_file.lock_exclusive() {
                Ok(_) => Ok(File::from_std(unlocked_file)),
                Err(e) => Err((unlocked_file, e)),
            };
            // println!("locked exclusive");
            result
        }));
        LockFuture { file_lock }
    }

    fn new_shared(file_lock: &'a mut FileLock) -> LockFuture<'a> {
        // println!("locking shared");
        let unlocked_file = file_lock.unlocked_file.take().unwrap();
        file_lock.locking_fut = Some(spawn_blocking(move || {
            let result = match unlocked_file.lock_shared() {
                Ok(_) => Ok(File::from_std(unlocked_file)),
                Err(e) => Err((unlocked_file, e)),
            };
            // println!("locked shared");
            result
        }));
        LockFuture { file_lock }
    }
}

impl<'a> Future for LockFuture<'a> {
    type Output = Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.file_lock.poll_locking_fut(cx)
    }
}

pub struct UnlockFuture<'a> {
    file_lock: &'a mut FileLock,
}

impl<'a> UnlockFuture<'a> {
    fn new(file_lock: &'a mut FileLock) -> UnlockFuture<'a> {
        file_lock.unlocking_fut = Some(file_lock.locked_file.take().unwrap().into_std().boxed());
        file_lock.state = State::Unlocking;
        UnlockFuture { file_lock }
    }
}

impl<'a> Future for UnlockFuture<'a> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.file_lock.poll_unlock(cx)
    }
}

