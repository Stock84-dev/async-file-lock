#![allow(unused_must_use)]
#![feature(test)]

extern crate test;

use test::Bencher;
use tempfile::NamedTempFile;
use tokio::fs::File;
use tokio::prelude::io::AsyncWriteExt;
use async_file_lock::FileLock;

#[bench]
fn tokio_write(b: &mut Bencher) {
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    let mut file = rt.block_on(async {
        File::create(NamedTempFile::new().unwrap().into_temp_path()).await.unwrap()
    });
    b.iter(|| {
        rt.block_on(async {
            file.write(b"a")
        });
    })
}

#[bench]
fn normal_write(b: &mut Bencher) {
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    let mut file = rt.block_on(async {
        let mut file = FileLock::create(NamedTempFile::new().unwrap().into_temp_path()).await.unwrap();
        file.lock_exclusive().await;
        file
    });
    b.iter(|| {
        rt.block_on(async {
            file.write(b"a")
        });
    })
}

#[bench]
fn auto_write(b: &mut Bencher) {
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    let mut file = rt.block_on(async {
        FileLock::create(NamedTempFile::new().unwrap().into_temp_path()).await.unwrap()
    });
    b.iter(|| {
        rt.block_on(async {
            file.write(b"a")
        });
    })
}
