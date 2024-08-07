use jwalk::DirEntry;
use rayon::ThreadPool;
use std::{
    borrow::Borrow,
    collections::HashSet,
    fs::{self, Metadata},
    io::{self, Read},
    ops::Sub,
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

pub struct Synchronize {
    src: PathBuf,
    dest: PathBuf,
    // Configuration
    delete: bool,
    num_threads: Option<u8>,
    skip_hidden: bool,
    display_progress: bool,
    check_content: bool,
    skip_permissions: bool,

    // Reporting
    progress: Progress,
}

#[derive(Debug, Default, Clone)]
struct DirState {
    is_error: bool,
    error: Arc<Mutex<Option<io::Error>>>,
}

type ClientState = (DirState, ());

impl Synchronize {
    pub fn new<A: Into<PathBuf>, B: Into<PathBuf>>(src: A, dest: B) -> Self {
        Self {
            src: src.into(),
            dest: dest.into(),
            delete: false,
            num_threads: None,
            skip_hidden: false,
            check_content: false,
            display_progress: false,
            skip_permissions: false,
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

    pub fn check_content(mut self, value: bool) -> Self {
        self.check_content = value;
        self
    }

    pub fn skip_permissions(mut self, value: bool) -> Self {
        self.skip_permissions = value;
        self
    }

    pub fn sync(self) -> anyhow::Result<()> {
        let sync = Arc::new(self);

        // Threadpool used by jwalk
        let thread_pool = Arc::new(sync.get_thread_pool()?);
        let parallelism = jwalk::Parallelism::RayonExistingPool {
            pool: thread_pool.clone(),
            busy_timeout: None,
        };

        // Read all source files and create the destination folder structure
        let sync_clone = sync.clone();
        let src_files = jwalk::WalkDirGeneric::<ClientState>::new(&sync_clone.src)
            .skip_hidden(sync_clone.skip_hidden)
            .parallelism(parallelism)
            .process_read_dir(move |depth, path, state, c| {
                if depth.is_none() {
                    return;
                }
                if state.is_error {
                    return;
                }
                match sync_clone.sync_dir(path, c) {
                    Ok(_) => {}
                    Err(e) => {
                        state.is_error = true;
                        state.error.lock().unwrap().replace(e);
                    }
                }
            });

        // Write symlinks
        src_files
            .into_iter()
            .map(|x| match x {
                Ok(x) => {
                    if x.path_is_symlink() {
                        return sync.sync_symlink(&x.path());
                    }
                    Ok(())
                }
                Err(e) => Err(anyhow::Error::msg(e.to_string())),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        sync.progress.print();

        Ok(())
    }

    fn sync_dir(
        &self,
        dir: &Path,
        children: &mut [jwalk::Result<DirEntry<ClientState>>],
    ) -> io::Result<()> {
        // Update progress
        self.progress.add_source(children.len());

        // Create destination directory if it doesn't already exist
        let dest = self.get_destination_path(dir);
        if !dest.exists() {
            match std::fs::create_dir(&dest) {
                Ok(_) => {}
                Err(e) => panic!("Failed to create directory {:?}: Error {:?}", &dest, e),
            }
            self.progress.add_copied(1);
        } else {
            self.progress.add_skipped(1);
        }

        let mut deletes = HashSet::new();
        if self.delete {
            deletes = fs::read_dir(dest)?
                .map(|x| x.map(|y| y.path()))
                .collect::<io::Result<HashSet<_>>>()?;
        }

        // Syncronize files
        for entry in children.iter_mut().flatten() {
            let pth = entry.path();
            let dest = self.get_destination_path(&pth);
            deletes.remove(&dest);
            if pth.is_file() && !pth.is_symlink() {
                match self.sync_file(&entry.path(), &dest) {
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

        for delete in deletes.into_iter() {
            self.remove_all(&delete)?;
        }

        Ok(())
    }

    fn sync_file(&self, src: &Path, dest: &Path) -> anyhow::Result<()> {
        let meta = src.symlink_metadata()?;
        let exists = dest.exists();

        if exists
            && (self.check_content && self.check_content_equal(src, dest).unwrap_or(false)
                || self.is_equal(&meta, dest).unwrap_or(false))
        {
            self.progress.add_skipped(1);
            return Ok(());
        }

        // Copy file data
        self.copy_file(&meta, src, dest)?;

        self.progress.add_copied(1);
        self.progress.add_bytes_copied(meta.len() as usize);

        // Preserve permissions
        if !self.skip_permissions {
            let perm = meta.permissions();
            std::fs::set_permissions(dest, perm)?;
        }

        // Preserve modified time
        let mtime = meta.modified()?;
        let atime = meta.accessed()?;
        filetime::set_file_times(dest, atime.into(), mtime.into())?;

        Ok(())
    }

    fn sync_symlink(&self, src: &Path) -> anyhow::Result<()> {
        let dest: PathBuf = self.get_destination_path(src);
        let link_path = std::fs::read_link(src)?;
        if dest.exists() {
            let meta = src.symlink_metadata()?;
            if !self.is_equal(&meta, &dest)? {
                return Ok(());
            }
            std::fs::remove_file(&dest)?;
        }
        match symlink(&link_path, &dest) {
            Err(e) => Err(anyhow::Error::msg(format!(
                "Failed to create symlink {:?} -> {:?} Error {:?}",
                src, dest, e
            ))),
            _ => Ok(()),
        }?;
        self.progress.add_copied(1);
        Ok(())
    }

    fn remove_all(&self, path: &Path) -> io::Result<()> {
        let filetype = fs::symlink_metadata(path)?.file_type();
        if filetype.is_symlink() || filetype.is_file() {
            fs::remove_file(path)?;
            self.progress.add_deleted(1);
            Ok(())
        } else {
            for child in fs::read_dir(path)? {
                let child = child?;
                if child.file_type()?.is_dir() {
                    self.remove_all(&child.path())?;
                } else {
                    fs::remove_file(child.path())?;
                    self.progress.add_deleted(1);
                }
            }
            Ok(())
        }
    }

    fn get_thread_pool(&self) -> anyhow::Result<ThreadPool> {
        let mut pool = rayon::ThreadPoolBuilder::new();
        if let Some(threads) = self.num_threads {
            pool = pool.num_threads(threads as usize)
        }
        let pool = pool.build()?;
        Ok(pool)
    }

    fn is_equal(&self, src_meta: &Metadata, dest_path: impl AsRef<Path>) -> anyhow::Result<bool> {
        let dest_meta = dest_path.as_ref().metadata()?;
        let same_l = dest_meta.len() == src_meta.len();
        let same_m = dest_meta.modified()? == src_meta.modified()?;
        Ok(same_l && same_m)
    }

    fn check_content_equal(
        &self,
        src: impl AsRef<Path>,
        dest: impl AsRef<Path>,
    ) -> anyhow::Result<bool> {
        let mut file1 = fs::File::open(src.as_ref())?;
        let mut file2 = fs::File::open(dest.as_ref())?;

        let mut buffer1 = [0; 1024]; // Using a buffer of 1024 bytes
        let mut buffer2 = [0; 1024];

        loop {
            let count1 = file1.read(&mut buffer1)?;
            let count2 = file2.read(&mut buffer2)?;

            if count1 != count2 || buffer1[..count1] != buffer2[..count2] {
                return Ok(false);
            }

            if count1 == 0 || count2 == 0 {
                break;
            }
        }

        Ok(true)
        //sz
    }
    fn get_destination_path(&self, src_path: &Path) -> PathBuf {
        let mut dest = self.dest.clone();
        dest.push(src_path.strip_prefix(&self.src).unwrap());
        dest
    }

    // File system utilities
    fn copy_file(&self, _meta: &Metadata, original: &Path, link: &Path) -> anyhow::Result<()> {
        match std::fs::copy(original, link) {
            Err(e) => Err(anyhow::Error::msg(format!(
                "Failed to copy file {:?} -> {:?} Error {:?}",
                link, original, e
            ))),
            _ => Ok(()),
        }
    }
}

#[derive(Debug)]
struct Progress {
    last_tick: Mutex<std::time::Instant>,
    start: std::time::Instant,
    paths: AtomicUsize,
    paths_deleted: AtomicUsize,
    paths_copied: AtomicUsize,
    paths_skipped: AtomicUsize,
    bytes_copied: AtomicUsize,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            last_tick: Mutex::new(std::time::Instant::now().sub(Duration::from_millis(120))),
            start: std::time::Instant::now(),
            paths: AtomicUsize::default(),
            paths_deleted: AtomicUsize::default(),
            paths_copied: AtomicUsize::default(),
            paths_skipped: AtomicUsize::default(),
            bytes_copied: AtomicUsize::default(),
        }
    }
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

    fn add_deleted(&self, bytes: usize) {
        self.paths_deleted.fetch_add(bytes, Ordering::Relaxed);
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

        if last_tick.elapsed() > Duration::from_millis(120) {
            *last_tick = std::time::Instant::now();
            self.print();
        }
    }

    fn print(&self) {
        let paths = self.paths.load(Ordering::Relaxed);
        let paths_copied = self.paths_copied.load(Ordering::Relaxed);
        let paths_skipped = self.paths_skipped.load(Ordering::Relaxed);
        let paths_deleted = self.paths_deleted.load(Ordering::Relaxed);
        let bytes_copied = self.bytes_copied.load(Ordering::Relaxed);
        let elapsed = self.start.elapsed();

        let del = match paths_deleted > 0 {
            true => format!("Deleted {:?} ", paths_deleted),
            false => "".to_string(),
        };

        eprint!(
            "\rFiles: {}, Copied: {}, Skipped: {}, Transfered {}, {}Elapsed: {:.2?} ",
            paths,
            paths_copied,
            paths_skipped,
            human_bytes::human_bytes(bytes_copied as f64),
            del,
            elapsed,
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
                file.write_all(&[b'a'; $file]).unwrap();
            }
        )+
        temp
    }};
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::temp_fs;

    use super::Synchronize;
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
            input / baz / foo / bar: 0,
            input / baz / foo / bean: 0,
        );
        let sync = Synchronize::new(temp.path().join("input"), temp.path().join("output"));
        sync.sync().unwrap();
        let paths = paths(jwalk::WalkDir::new(temp.path().join("output")), temp.path());
        assert_eq!(
            paths,
            vec![
                "output".to_string(),
                "output/baz".to_string(),
                "output/baz/foo".to_string(),
                "output/baz/foo/bean.text".to_string(),
                "output/baz/foo/bar.text".to_string(),
                "output/bar.text".to_string(),
            ]
        );
    }
}
