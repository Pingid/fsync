use jwalk::DirEntry;
use rayon::ThreadPool;
use std::{
    borrow::Borrow,
    fs::Metadata,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::symlink;

#[cfg(windows)]
use std::os::windows::fs::symlink_file as symlink;

pub struct Syncronize {
    src: PathBuf,
    dest: PathBuf,
    // Configuration
    delete: bool,
    num_threads: Option<u8>,
    skip_hidden: bool,
    display_progress: bool,

    // Reporting
    progress: Progress,
}

impl Syncronize {
    pub fn new<A: Into<PathBuf>, B: Into<PathBuf>>(src: A, dest: B) -> Self {
        Self {
            src: src.into(),
            dest: dest.into(),
            delete: false,
            num_threads: None,
            skip_hidden: false,
            display_progress: false,
            progress: Progress::default(),
        }
    }

    pub fn delete(mut self, value: bool) -> Self {
        self.delete = value;
        self
    }

    pub fn num_threads(mut self, value: Option<u8>) -> Self {
        self.num_threads = value;
        self
    }

    pub fn skip_hidden(mut self, value: bool) -> Self {
        self.skip_hidden = value;
        self
    }

    pub fn display_progress(mut self, value: bool) -> Self {
        self.display_progress = value;
        self
    }

    pub fn sync(self) -> anyhow::Result<()> {
        let sync = Arc::new(self);

        // Threadpool used by jwalk
        let thread_pool = Arc::new(sync.get_thread_pool()?);

        // Read all source files and create the destination folder structure
        let sync_clone = sync.clone();
        let src_files = jwalk::WalkDir::new(&sync_clone.src)
            .skip_hidden(sync_clone.skip_hidden)
            .parallelism(jwalk::Parallelism::RayonExistingPool {
                pool: thread_pool.clone(),
                busy_timeout: None,
            })
            .process_read_dir(move |depth, path, _, c| {
                if depth.is_none() {
                    return;
                }
                sync_clone.sync_dir(&path, c);
            });

        // Collect source files
        src_files
            .into_iter()
            .map(|x| match x {
                Ok(_) => Ok(()),
                Err(e) => Err(anyhow::Error::msg(e.to_string())),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(())
    }

    fn sync_dir(&self, dir: &Path, children: &mut Vec<jwalk::Result<DirEntry<((), ())>>>) {
        // Update progress
        self.progress.add_source(children.len());

        // Create destination directory if it doesn't already exist
        let dest = self.get_destination_path(&dir);
        if !dest.exists() {
            match std::fs::create_dir(&dest) {
                Ok(_) => {}
                Err(e) => panic!("Failed to create directory {:?}: Error {:?}", &dest, e),
            }
            self.progress.add_copied(1);
        } else {
            self.progress.add_skipped(1);
        }

        // Syncronize files
        for entry in children {
            if let Ok(entry) = entry {
                if entry.path().is_file() {
                    match self.sync_file(&entry.path()) {
                        Ok(_) => {}
                        Err(e) => {
                            self.progress.println(format!(
                                "Error syncing {:?}: {:?}",
                                &entry.path(),
                                e
                            ));
                            entry.read_children_path = None;
                        }
                    }
                }
            }
        }
    }

    fn sync_file(&self, src: &Path) -> anyhow::Result<()> {
        let meta = src.symlink_metadata()?;
        let dest = self.get_destination_path(&src);
        let exists = dest.exists();

        if exists && self.is_equal(&meta, &dest)? {
            self.progress.add_skipped(1);
            return Ok(());
        }

        // Write symlink
        if meta.is_symlink() {
            let link_path = std::fs::read_link(&src)?;
            if exists {
                std::fs::remove_file(&dest)?;
            }
            self.symlink(&link_path, &dest)?;
        } else {
            self.copy_file(&meta, &src, &dest)?;
        }

        self.progress.add_copied(1);
        self.progress.add_bytes_copied(meta.len() as usize);

        // Preserve permissions
        let perm = meta.permissions();
        std::fs::set_permissions(&dest, perm)?;

        // Preserve modified time
        let mtime = meta.modified()?;
        let atime = meta.accessed()?;
        filetime::set_file_times(&dest, atime.into(), mtime.into())?;

        Ok(())
    }

    fn get_thread_pool(&self) -> anyhow::Result<ThreadPool> {
        let mut pool = rayon::ThreadPoolBuilder::new();
        if let Some(threads) = self.num_threads {
            pool = pool.num_threads(threads as usize)
        }
        let pool = pool.build()?;
        Ok(pool)
    }

    fn is_equal(&self, src_meta: &Metadata, dest_path: &Path) -> anyhow::Result<bool> {
        let dest_meta = dest_path.metadata()?;
        let same_l = dest_meta.len() == src_meta.len();
        let same_m = dest_meta.modified()? == src_meta.modified()?;
        Ok(same_l && same_m)
    }

    fn get_destination_path(&self, src_path: &Path) -> PathBuf {
        let mut dest = self.dest.clone();
        dest.push(src_path.strip_prefix(&self.src).unwrap());
        dest
    }

    // File system utilities
    fn copy_file(&self, _meta: &Metadata, original: &Path, link: &Path) -> anyhow::Result<()> {
        match std::fs::copy(&original, &link) {
            Err(e) => Err(anyhow::Error::msg(format!(
                "Failed to copy file {:?} -> {:?} Error {:?}",
                link, original, e
            ))),
            _ => Ok(()),
        }
    }

    fn symlink(&self, src: &Path, dest: &Path) -> anyhow::Result<()> {
        match symlink(src, dest) {
            Err(e) => Err(anyhow::Error::msg(format!(
                "Failed to create symlink {:?} -> {:?} Error {:?}",
                src, dest, e
            ))),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Default)]
struct Progress {
    last_tick: Mutex<Option<std::time::Instant>>,
    paths: AtomicUsize,
    paths_copied: AtomicUsize,
    paths_skipped: AtomicUsize,
    bytes_copied: AtomicUsize,
}

impl Progress {
    fn add_source(&self, bytes: usize) {
        self.paths.fetch_add(bytes, Ordering::Relaxed);
        self.tick();
    }

    fn add_copied(&self, bytes: usize) {
        self.paths_copied.fetch_add(bytes, Ordering::Relaxed);
        self.tick();
    }

    fn add_skipped(&self, bytes: usize) {
        self.paths_skipped.fetch_add(bytes, Ordering::Relaxed);
        self.tick();
    }

    fn add_bytes_copied(&self, bytes: usize) {
        self.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
        self.tick();
    }

    fn println<S: Borrow<str>>(&self, s: S) {
        eprintln!("\r{}", s.borrow());
        self.print();
    }

    fn tick(&self) {
        let mut last_tick = self.last_tick.lock().unwrap();
        if let Some(last) = *last_tick {
            if last.elapsed() > Duration::from_millis(120) {
                *last_tick = Some(std::time::Instant::now());
                self.print();
            }
        } else {
            *last_tick = Some(std::time::Instant::now());
            self.print();
        }
    }

    fn print(&self) {
        let paths = self.paths.load(Ordering::Relaxed);
        let paths_copied = self.paths_copied.load(Ordering::Relaxed);
        let paths_skipped = self.paths_skipped.load(Ordering::Relaxed);
        let bytes_copied = self.bytes_copied.load(Ordering::Relaxed);

        eprint!(
            "\rFiles: {}, Copied: {}, Skipped: {}, Transfered {}",
            paths,
            paths_copied,
            paths_skipped,
            human_bytes::human_bytes(bytes_copied as f64)
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::temp_fs;

    use super::Syncronize;
    use jwalk::WalkDir;

    pub fn paths<P: AsRef<Path>>(walk: WalkDir, rel: P) -> Vec<String> {
        walk.into_iter()
            .map(|x| {
                x.unwrap()
                    .path()
                    .strip_prefix(&rel)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn test_example() {
        let temp = temp_fs!(
            input / bar: 0,
            input / baz / foo / bar: 0
        );
        let sync = Syncronize::new(temp.path().join("input"), temp.path().join("output"));
        sync.sync().unwrap();
        let paths = paths(jwalk::WalkDir::new(temp.path().join("output")), temp.path());
        assert_eq!(
            paths,
            vec![
                "output".to_string(),
                "output/baz".to_string(),
                "output/baz/foo".to_string(),
                "output/baz/foo/bar.text".to_string(),
                "output/bar.text".to_string()
            ]
        );
    }
}

#[macro_export]
macro_rules! temp_fs {
    ($($($dir:ident)/+: $file:expr),+ $(,)?) => {{
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        $(
            {
            let path = concat!($(stringify!($dir), "/",)+);
            let path = temp.path().join(format!("{}.text", &path[0..path.len() - 1]));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut file = std::fs::File::create(&path).unwrap();
            file.write(&vec!['a' as u8; $file]).unwrap();
            }
        )+
        temp
    }};
}
