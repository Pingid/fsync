# fsync
A multi-threaded utility designed to synchronize directories, offering significant speed improvements over traditional methods like rsync by utilizing multiple threads. This tool is particularly useful for backing up files across different directories efficiently.

## Installation
Install fsync using Cargo by running the following command:
```
cargo install --git https://github.com/Pingid/fsync
```

## Usage
To use Fsync, specify the source and destination directories along with any desired options.
```
Usage: fsync [OPTIONS] <source> <destination>

Arguments:
  <source>       Source directory
  <destination>  Destination directory

Options:
  -d, --delete             Delete files in the destination that are not in the source
      --threads <threads>  Number of threads to use defaults to rayon default threadpool
  -h, --help               Print help
```
