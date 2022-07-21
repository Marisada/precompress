use std::{
    cmp::max,
    fs::File,
    path::{Path, PathBuf},
    thread::spawn,
    time::{Duration, Instant},
};

use crossbeam::channel::{bounded, Receiver, Sender};

use crate::encode::{Context, Quality};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Algorithm {
    Brotli,
    Deflate,
    Gzip,
    Zstd,
}

impl Algorithm {
    fn extension(self) -> &'static str {
        match self {
            Self::Brotli => ".br",
            Self::Deflate => ".zz",
            Self::Gzip => ".gz",
            Self::Zstd => ".zst",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Algorithms {
    pub(crate) brotli: bool,
    pub(crate) deflate: bool,
    pub(crate) gzip: bool,
    pub(crate) zstd: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Stats {
    pub(crate) num_files: u64,
    pub(crate) num_errors: u64,

    pub(crate) brotli_time: Duration,
    pub(crate) deflate_time: Duration,
    pub(crate) gzip_time: Duration,
    pub(crate) zstd_time: Duration,
}

impl std::ops::Add<Stats> for Stats {
    type Output = Stats;

    fn add(self, rhs: Stats) -> Stats {
        Stats {
            num_files: self.num_files + rhs.num_files,
            num_errors: self.num_errors + rhs.num_errors,
            brotli_time: self.brotli_time + rhs.brotli_time,
            deflate_time: self.deflate_time + rhs.deflate_time,
            gzip_time: self.gzip_time + rhs.gzip_time,
            zstd_time: self.zstd_time + rhs.zstd_time,
        }
    }
}

pub(crate) struct Compressor {
    threads: usize,
    quality: Quality,
    algorithms: Algorithms,
}

type Unit = (Algorithm, PathBuf);

impl Compressor {
    pub(crate) fn new(threads: usize, quality: Quality, algorithms: Algorithms) -> Self {
        Compressor {
            threads,
            quality,
            algorithms,
        }
    }

    pub(crate) fn precompress(&self, path: PathBuf) -> Stats {
        let cap = max(self.threads * 2, 64);
        let (tx, rx): (Sender<Unit>, Receiver<Unit>) = bounded(cap);

        let quality = self.quality;
        let mut handles = Vec::with_capacity(self.threads);
        for _ in 0..self.threads {
            let rx = rx.clone();
            handles.push(spawn(move || Compressor::worker(rx, quality)));
        }

        let walk = ignore::WalkBuilder::new(&path)
            .ignore(false)
            .git_exclude(false)
            .git_global(false)
            .git_ignore(false)
            .follow_links(false)
            .build();
        for entry in walk {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    eprintln!("Warning: {}", err);
                    continue;
                }
            };
            let path = entry.path();
            if should_compress(path) && !path.is_symlink() && path.is_file() {
                if self.algorithms.brotli {
                    let path = path.to_path_buf();
                    tx.send((Algorithm::Brotli, path)).expect("channel send");
                }
                if self.algorithms.deflate {
                    let path = path.to_path_buf();
                    tx.send((Algorithm::Deflate, path)).expect("channel send");
                }
                if self.algorithms.gzip {
                    let path = path.to_path_buf();
                    tx.send((Algorithm::Gzip, path)).expect("channel send");
                }
                if self.algorithms.zstd {
                    let path = path.to_path_buf();
                    tx.send((Algorithm::Zstd, path)).expect("channel send");
                }
            }
        }
        drop(tx);

        let mut stats = Stats::default();
        for handle in handles {
            let h_stats = handle.join().expect("unable to join worker thread");
            stats = stats + h_stats;
        }
        stats
    }

    fn worker(rx: Receiver<Unit>, quality: Quality) -> Stats {
        let mut stats = Stats::default();
        let mut ctx = Context::new(1 << 14, quality);

        while let Ok((algorithm, pathbuf)) = rx.recv() {
            let start = Instant::now();
            if let Err(err) = Compressor::encode_file(&mut ctx, algorithm, &pathbuf) {
                eprintln!("Warning: {}: {}", pathbuf.display(), err);
                stats.num_errors += 1;
            } else {
                let dur = start.elapsed();
                match algorithm {
                    Algorithm::Brotli => stats.brotli_time += dur,
                    Algorithm::Deflate => stats.deflate_time += dur,
                    Algorithm::Gzip => stats.gzip_time += dur,
                    Algorithm::Zstd => stats.zstd_time += dur,
                }
                stats.num_files += 1;
            }
        }

        stats
    }

    fn encode_file(ctx: &mut Context, alg: Algorithm, path: &PathBuf) -> anyhow::Result<()> {
        let mut src = File::open(path)?;

        let mut file_name = match path.file_name() {
            None => return Ok(()),
            Some(name) => name,
        }
        .to_os_string();
        file_name.push(alg.extension());
        let dst_path = path.with_file_name(file_name);

        let mut dst = File::create(dst_path)?;
        match alg {
            Algorithm::Brotli => ctx.write_brotli(&mut src, &mut dst)?,
            Algorithm::Deflate => ctx.write_deflate(&mut src, &mut dst)?,
            Algorithm::Gzip => ctx.write_gzip(&mut src, &mut dst)?,
            Algorithm::Zstd => ctx.write_zstd(&mut src, &mut dst)?,
        };
        Ok(())
    }
}

fn should_compress(path: &Path) -> bool {
    if let Some(ext) = path.extension() {
        if let Some(ext) = ext.to_str() {
            return EXTENSIONS.contains(ext);
        }
    }
    false
}

static EXTENSIONS: phf::Set<&'static str> = phf::phf_set! {
    "atom",
    "conf",
    "css",
    "eot",
    "htm",
    "html",
    "js",
    "json",
    "jsx",
    "md",
    "otf",
    "rss",
    "scss",
    "sitemap",
    "svg",
    "text",
    "ts",
    "tsx",
    "ttf",
    "txt",
    "wasm",
    "xml",
    "yaml",
};
