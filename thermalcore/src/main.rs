// Thin runner: the monitor loop lives in the `rianixia_thermalcore` lib so the
// unified sys.azenith-service can start it in-process (plan Q1, Option A).
// thermalcore keeps its own binary target either way — the daemon does not hold
// a thermal loop, `service.sh` still execs this.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::ExitCode::from(rianixia_thermalcore::run(&args) as u8)
}
