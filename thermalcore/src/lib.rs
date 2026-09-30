pub mod android_ffi;
pub mod constants;
pub mod context;
pub mod cooling;
pub mod cpu;
pub mod effectiveness;
pub mod learning;
pub mod monitor;
pub mod policy_manager;
pub mod state;
pub mod thermal_zones;
pub mod utils;

#[cfg(feature = "simulator")]
pub mod simulator;

pub use monitor::ThermalMonitor;
pub use android_ffi::Logger;
pub use learning::ThermalAI;

/// Runs the thermal monitor until SIGTERM/SIGINT. Moved out of `main` so the
/// unified `sys.azenith-service` can start it in-process (plan Q1).
///
/// Returns a process exit code: 0 on a clean shutdown, 1 on a fatal runtime
/// error. `args` carries `--simulate <trace>` when the `simulator` feature is on.
use std::sync::{ atomic::{ AtomicBool }, Arc };


// Without the `simulator` feature there is no subcommand to parse, hence the
// underscore — see the `#[cfg]`-gated use inside.
#[cfg_attr(not(feature = "simulator"), allow(unused_variables))]
pub fn run(args: &[String]) -> i32 {
    #[cfg(feature = "simulator")]
    {
        if args.first().map(String::as_str) == Some("--simulate") {
            let Some(trace) = args.get(1) else {
                eprintln!("Usage: {} --simulate <trace_file.json>", args[0]);
                return 1;
            };
            match simulator::ThermalSimulator::new(trace) {
                Ok(mut sim) => {
                    sim.run_simulation();
                    return 0;
                }
                Err(e) => {
                    eprintln!("Failed to initialize simulator: {}", e);
                    return 1;
                }
            }
        }
    }

    let term_flag = Arc::new(AtomicBool::new(false));
    if
        let Err(e) = signal_hook::flag::register(
            signal_hook::consts::SIGTERM,
            Arc::clone(&term_flag)
        )
    {
        eprintln!("Failed to register SIGTERM: {}", e);
    }
    if
        let Err(e) = signal_hook::flag::register(
            signal_hook::consts::SIGINT,
            Arc::clone(&term_flag)
        )
    {
        eprintln!("Failed to register SIGINT: {}", e);
    }

    let mut monitor = ThermalMonitor::new();

    if let Err(e) = monitor.run(term_flag) {
        monitor.logger.error(&format!("Fatal runtime error: {}", e));
        monitor.logger.info("Saving data on fatal error...");

        if let Err(save_e) = monitor.learning_data.save() {
            monitor.logger.error(
                &format!("Failed to save learning data on error exit: {}", save_e)
            );
        }
        return 1;
    }

    monitor.logger.info("Rianixia Thermal Core shutting down.");

    // monitor.user_pattern_tracker.finalize_session();

    if let Err(save_e) = monitor.learning_data.save() {
        monitor.logger.error(&format!("Failed to save data on clean exit: {}", save_e));
    }
    monitor.logger.info("Save complete. Exiting.");
    0
}
