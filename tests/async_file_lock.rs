#![deny(unused_must_use)]

use async_file_lock::FileLock;
use tokio::test;
use std::io::Result;
use tempfile::NamedTempFile;
use tokio::io::{AsyncWriteExt, SeekFrom, AsyncSeekExt, AsyncReadExt};
use std::time::Instant;
use fork::{fork, Fork};

fn file() -> FileLock {
    FileLock::new_std(tempfile::tempfile().unwrap())
}

#[test(threaded_scheduler)]
async fn normal1() -> Result<()> {
    let tmp_path = NamedTempFile::new()?.into_temp_path();
    // let tmp_path = PathBuf::from("/tmp/a");
    match fork() {
        Ok(Fork::Parent(_)) => {
            // println!("parent {}", tmp_path.exists());
            let mut buf = String::new();
            let mut file = FileLock::create(&tmp_path).await?;
            // println!("parent {}", tmp_path.exists());
            println!("written {}", file.write(b"a").await?);
            std::thread::sleep(std::time::Duration::from_millis(200));
            println!("slept");
            println!("sought {}", file.seek(SeekFrom::Start(0)).await?);
            file.read_to_string(&mut buf).await?;
            println!("read");
            // // We write at location 0 then it gets overridden.
            assert_eq!(buf, "b");
        }
        Ok(Fork::Child) => {
            std::thread::sleep(std::time::Duration::from_millis(100));
            println!("child {}", tmp_path.exists());
            let mut file = FileLock::open(&tmp_path).await?;
            println!("child opened");
            file.write(b"b").await?;
            println!("done");
            std::process::exit(0);
        },
        Err(_) => panic!("unable to fork"),
    }
    Ok(())
}

#[test(threaded_scheduler)]
async fn append_mode() -> Result<()> {
    let tmp_path = NamedTempFile::new()?.into_temp_path();
    match fork() {
        Ok(Fork::Parent(_)) => {
            let mut buf = String::new();
            let mut file = FileLock::create(&tmp_path).await?;
            file.set_seeking_mode(SeekFrom::End(0));
            file.write(b"a").await?;
            // println!("parent {}", tmp_path.exists());
            std::thread::sleep(std::time::Duration::from_millis(200));
            file.write(b"a").await?;
            file.seek(SeekFrom::Start(0)).await?;
            // Turn off auto seeking mode
            file.set_seeking_mode(SeekFrom::Current(0));
            file.read_to_string(&mut buf).await?;
            // Each file handle has its own position.
            assert_eq!(buf, "bba");
        }
        Ok(Fork::Child) => {
            std::thread::sleep(std::time::Duration::from_millis(100));
            // println!("{}", tmp_path.exists());
            let mut file = FileLock::open(&tmp_path).await?;
            file.write(b"bb").await?;
            file.flush().await?;
            // println!("done");
            std::process::exit(0);
        },
        Err(_) => panic!("unable to fork"),
    }
    Ok(())
}

#[test(threaded_scheduler)]
async fn lock_shared() -> Result<()> {
    let tmp_path = NamedTempFile::new()?.into_temp_path();
    match fork() {
        Ok(Fork::Parent(_)) => {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut file = FileLock::open(&tmp_path).await?;
            let instant = Instant::now();
            file.lock_shared().await?;
            assert!(instant.elapsed().as_millis() < 100);
        }
        Ok(Fork::Child) => {
            let mut file = FileLock::open(&tmp_path).await?;
            file.lock_shared().await?;
            std::thread::sleep(std::time::Duration::from_millis(1000));
            std::process::exit(0);
        },
        Err(_) => panic!("unable to fork"),
    }
    Ok(())
}


#[test(threaded_scheduler)]
async fn lock_exclusive() -> Result<()> {
    let tmp_path = NamedTempFile::new()?.into_temp_path();
    match fork() {
        Ok(Fork::Parent(_)) => {
            std::thread::sleep(std::time::Duration::from_millis(100));
            println!("parent {}", tmp_path.exists());
            let mut file = FileLock::open(&tmp_path).await?;
            let instant = Instant::now();
            file.lock_exclusive().await?;
            assert!(instant.elapsed().as_millis() > 800);
        }
        Ok(Fork::Child) => {
            let mut file = FileLock::create(&tmp_path).await?;
            file.lock_exclusive().await?;
            println!("child {}", tmp_path.exists());
            std::thread::sleep(std::time::Duration::from_millis(1000));
            std::process::exit(0);
        },
        Err(_) => panic!("unable to fork"),
    }
    Ok(())
}

#[test(threaded_scheduler)]
async fn lock_exclusive_shared() -> Result<()> {
    let tmp_path = NamedTempFile::new()?.into_temp_path();
    match fork() {
        Ok(Fork::Parent(_)) => {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut file = FileLock::open(&tmp_path).await?;
            let instant = Instant::now();
            file.lock_exclusive().await?;
            assert!(instant.elapsed().as_millis() > 800);
        }
        Ok(Fork::Child) => {
            let mut file = FileLock::open(&tmp_path).await?;
            file.lock_shared().await?;
            std::thread::sleep(std::time::Duration::from_millis(1000));
            std::process::exit(0);
        },
        Err(_) => panic!("unable to fork"),
    }
    Ok(())
}

#[test(threaded_scheduler)]
async fn drop_lock_exclusive() -> Result<()> {
    let tmp_path = NamedTempFile::new()?.into_temp_path();
    match fork() {
        Ok(Fork::Parent(_)) => {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut file = FileLock::open(&tmp_path).await?;
            let instant = Instant::now();
            file.lock_exclusive().await?;
            assert!(instant.elapsed().as_millis() < 100);
        }
        Ok(Fork::Child) => {
            let mut file = FileLock::open(&tmp_path).await?;
            file.lock_exclusive().await?;
            drop(file);
            std::thread::sleep(std::time::Duration::from_millis(1000));
            std::process::exit(0);
        },
        Err(_) => panic!("unable to fork"),
    }
    Ok(())
}

#[test(threaded_scheduler)]
#[should_panic]
async fn exclusive_locking_locked_file_panics() {
    let mut file = file();
    file.lock_exclusive().await.unwrap();
    file.lock_exclusive().await.unwrap();
}

#[test(threaded_scheduler)]
#[should_panic]
async fn shared_locking_locked_file_panics() {
    let mut file = file();
    file.lock_shared().await.unwrap();
    file.lock_shared().await.unwrap();
}

#[test(threaded_scheduler)]
#[should_panic]
async fn shared_locking_exclusive_file_panics() {
    let mut file = file();
    file.lock_exclusive().await.unwrap();
    file.lock_shared().await.unwrap();
}

#[test(threaded_scheduler)]
#[should_panic]
async fn exclusive_locking_shared_file_panics() {
    let mut file = file();
    file.lock_shared().await.unwrap();
    file.lock_exclusive().await.unwrap();
}

#[test(threaded_scheduler)]
#[should_panic]
async fn unlocking_file_panics() {
    let mut file = file();
    file.unlock().await;
}

#[test(threaded_scheduler)]
#[should_panic]
async fn unlocking_unlocked_file_panics() {
    let mut file = file();
    file.lock_exclusive().await.unwrap();
    file.unlock().await;
    file.unlock().await;
}

#[test(threaded_scheduler)]
async fn file_stays_locked() {
    let mut file = file();
    file.lock_exclusive().await.unwrap();
    file.write(b"a").await.unwrap();
    file.unlock().await;
}

#[test(threaded_scheduler)]
#[should_panic]
async fn file_auto_unlocks() {
    let mut file = file();
    file.write(b"a").await.unwrap();
    file.unlock().await;
}