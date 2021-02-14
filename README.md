# async-file-lock
Asynchronous file lock that can auto lock and auto seek.

# Features
- async file locks (exclusive and shared)
- auto lock before doing any read or write operation
- auto seek before doing any read or write operation
- manual lock/unlock

# Platforms
async-file-lock should work on any platform supported by libc.

# example
```rust
use async_file_lock::FileLock;
use tokio::io::{AsyncWriteExt, SeekFrom, AsyncSeekExt, AsyncReadExt};
//...

// Create file in read/write mode
let mut file = FileLock::create(&tmp_path).await?;
// Supports any methods from AsyncReadExt, AsyncWriteExt, AsyncSeekExt
// Locks exclusive
file.write(b"a").await?;
// Releases lock
file.seek(SeekFrom::Start(0)).await?;
let mut string = String::new();
// Locks shared
file.read_to_string(&mut string).await?;
// Releases lock
file.seek(SeekFrom::Start(0)).await?;
// Locks exclusive and holds
file.lock_exclusive().await?;
file.set_seeking_mode(SeekFrom::End(0));
// Seek to end and write
file.write(b"b").await?;
file.seek(SeekFrom::Start(0)).await?;
// Seek to end and write
file.write(b"c").await?;
// Finally releases lock
file.unlock().await;
file.lock_shared().await?;
// Cursor is at the end of a file, we want to read whole file.
file.seek(SeekFrom::Start(0)).await?;
// Removing seeking mode, otherwise before reading cursor will seek
// to the end of a file.
file.set_seeking_mode(SeekFrom::Current(0));
string.clear();
file.read_to_string(&mut string).await?;
assert_eq!(string, "abc");
```
