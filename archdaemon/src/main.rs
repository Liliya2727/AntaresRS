// libBinding — AZenith status & profile daemon, pure binder (no shell fork).
// Module layout follows encore/jni/: ndk (BinderNDK), codes+status (BinderMonitor),
// observer (BinderMonitor/ProcessUtility), applist (GameRegistry/InotifyWatcher), log (EncoreLog).
mod applist;
mod codes;
mod log;
mod ndk;
mod observer;
mod state;
mod status;

use std::fs;
use std::ptr::null_mut;

use crate::applist::{reload_list, watch_list, LIST_DIR};
use crate::codes::{codes, load_codes, set_codes};
use crate::log::{debug, error, info, level_name, log_init, warn};
use crate::ndk::{call, class, n, service};
use crate::observer::{on_am_died, on_observer, set_pm};
use crate::state::adopt;
use crate::status::init_status;

pub(crate) const OUT_DIR: &str = "/data/adb/.libBinding";

fn main() {
    if unsafe { libc::getuid() } != 0 { eprintln!("run as root"); std::process::exit(1); }
    let _ = fs::create_dir_all(OUT_DIR);
    let _ = fs::create_dir_all(LIST_DIR);
    log_init();
    info!("libBinding start (pid {}, log level {})", std::process::id(), level_name());

    let Some(c) = load_codes() else {
        error!("failed to resolve transaction codes (check binder_resolver.apk)");
        std::process::exit(1);
    };
    set_codes(c);

    reload_list(); // initial load
    watch_list();  // auto reload when the file changes

    let n = n();
    let am = service(c"activity", c"android.app.IActivityManager");
    if am.is_null() { std::process::exit(1); }

    let pm = service(c"package", c"android.content.pm.IPackageManager");
    if pm.is_null() { std::process::exit(1); }
    set_pm(pm);

    unsafe {
        let rec = (n.AIBinder_DeathRecipient_new)(on_am_died);
        let r = (n.AIBinder_linkToDeath)(am, rec, null_mut());
        if r == 0 { debug!("linkToDeath registered on ActivityManager"); }
        else { warn!("linkToDeath failed, status={r}"); }
    }

    let obs = unsafe { (n.AIBinder_new)(class(c"android.app.IProcessObserver", on_observer), null_mut()) };
    if obs.is_null() { error!("failed to create observer binder"); std::process::exit(1); }

    let Some(reply) = call(am, codes().reg, |i| unsafe {
        (n.AParcel_writeStrongBinder)(i, obs);
    }) else {
        error!("registerProcessObserver denied/failed (wrong code for this ROM?)");
        std::process::exit(1);
    };
    unsafe { (n.AParcel_delete)(reply); }
    info!("IProcessObserver registered with ActivityManager");

    unsafe { (n.ABinderProcess_startThreadPool)(); }
    init_status(); // register binder callbacks (screen/saver/zen) + read initial status
    adopt("start"); // write initial state (2/NULL, or 1 if a game is on screen)
    info!("ready, waiting for callbacks from AMS");
    unsafe { (n.ABinderProcess_joinThreadPool)(); }
}
