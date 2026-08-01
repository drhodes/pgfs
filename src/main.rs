mod db;
mod error;
mod fs;

use clap::Parser;
use db::Db;
use error::Result;
use fs::PgFs;

/// pgfs — mount a directory tree of files backed by a Postgres table.
///
/// This is the smallest useful version: arbitrary subdirectories, whole
/// files stored as blobs, no chunked blocks, no search. It exists to prove
/// the read/write path end to end before any of that gets layered on.
#[derive(Parser)]
struct Args {
    /// Where to mount the filesystem, e.g. ./testdata/mnt
    mountpoint: String,

    /// libpq-style connection string. Defaults to a Unix socket inside
    /// this project's testdata directory -- never a system-wide Postgres
    /// install or data directory. See scripts/init_db.sh to set that up.
    #[arg(long, default_value_t = format!("host={}/testdata dbname=pgfs", std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()))]
    conn: String,
}

fn main() {
    env_logger::init();
    let args = Args::parse();
    if let Err(err) = run(&args) {
        // {:#} prints the whole chain: what failed, where, and the cause.
        eprintln!("pgfs: {err:#}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> Result<()> {
    // Db::connect already tags its own failure with a full story (see db.rs).
    let db = Db::connect(&args.conn)?;

    // AutoUnmount is the reliability guarantee: the kernel removes the mount
    // whenever this process exits for any reason (clean exit, SIGKILL, crash),
    // so a dead daemon can never leave a ghost mount behind. It requires
    // allow_other (fuser adds it automatically), which in turn requires
    // user_allow_other in /etc/fuse.conf — see scripts/pgfs.sh.
    let options = vec![
        fuser::MountOption::FSName("pgfs".to_string()),
        fuser::MountOption::AutoUnmount,
    ];

    println!("mounting pgfs at {}", args.mountpoint);
    let mut session = error::ctx(
        fuser::Session::new(PgFs::new(db), std::path::Path::new(&args.mountpoint), &options),
        &format!("mount at {}", args.mountpoint),
    )?;

    // Block SIGINT/SIGTERM/SIGHUP process-wide and wait for them on a
    // dedicated thread. On receipt we unmount cleanly, which makes run()
    // return instead of the process dying and relying on AutoUnmount alone.
    let signals = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
    let mut unmounter = session.unmount_callable();
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(&mut set) != 0
            || signals
                .iter()
                .any(|s| libc::sigaddset(&mut set, *s) != 0)
        {
            log::warn!("could not build the shutdown signal set; Ctrl+C will fall back to AutoUnmount");
        } else if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            log::warn!("could not block shutdown signals; Ctrl+C will fall back to AutoUnmount");
        }
    }
    std::thread::spawn(move || unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in &signals {
            libc::sigaddset(&mut set, *s);
        }
        let mut sig = 0;
        if libc::sigwait(&set, &mut sig) == 0 {
            log::info!("received signal {sig}; unmounting cleanly");
            if let Err(e) = unmounter.unmount() {
                log::error!("clean unmount failed (AutoUnmount will still clean up on exit): {e}");
            }
        } else {
            log::error!("sigwait failed: {}", std::io::Error::last_os_error());
        }
    });

    error::ctx(session.run(), "serve FUSE requests")?;
    Ok(())
}
