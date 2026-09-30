use clap::{Parser, Subcommand};
use vectis_common::dbus_api::{DaemonProxy, OBJECT_PATH};
use vectis_common::types::DaemonStatus;
use zbus::Connection;

#[derive(Parser)]
#[command(name = "vectis", version, about = "Control TDP/GPU power and switch power profiles via vectisd")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Apply a named power profile (powersave | balanced | performance)
    Profile { name: String },

    /// Set a hard RAPL TDP cap
    Tdp {
        /// Sustained power limit (PL1), watts
        #[arg(long)]
        pl1: Option<u32>,
        /// Short-term boost power limit (PL2), watts
        #[arg(long)]
        pl2: Option<u32>,
        /// Boost window duration, microseconds
        #[arg(long)]
        tau: Option<u32>,
        /// Force pl2 == pl1, removing the boost window entirely
        #[arg(long)]
        hard_lock: bool,
    },

    /// Reset TDP to the hardware's own default limits
    TdpClear,

    /// Switch GPU mode: integrated | discrete | hybrid
    Gpu {
        mode: String,
        /// Force open-source driver (nouveau on nvidia; no-op on AMD)
        #[arg(short = 'n', long = "nouveau")]
        nouveau: bool,
        /// Apply immediately instead of queueing for next logout/boot
        #[arg(short, long)]
        force: bool,
    },

    /// Cancel a queued GPU switch, so it is not applied on the next logout
    GpuCancel,

    /// Full status: profile, tdp, gpu, conflicts
    Status {
        /// Print raw JSON instead of human-readable summary
        #[arg(long)]
        json: bool,
    },

    /// Raw PCI display-class device listing
    GpuList,

    /// Services from the conflict watch-list that are currently active
    Conflicts,
}

async fn proxy(conn: &Connection) -> zbus::Result<DaemonProxy<'_>> {
    DaemonProxy::builder(conn).path(OBJECT_PATH)?.build().await
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let conn = match Connection::system().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to connect to system bus: {e}");
            std::process::exit(1);
        }
    };

    let p = match proxy(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to reach vectisd (is it running? sudo systemctl status vectisd): {e}");
            std::process::exit(1);
        }
    };

    let result = match cli.command {
        Command::Profile { name } => {
            p.apply_profile(&name).await.map(|_| println!("applied profile: {name}"))
        }
        Command::Tdp { pl1, pl2, tau, hard_lock } => p
            .set_tdp(pl1.unwrap_or(0), pl2.unwrap_or(0), tau.unwrap_or(0), hard_lock)
            .await
            .map(|_| println!("tdp updated")),
        Command::TdpClear => p.clear_tdp().await.map(|_| println!("tdp reset to hardware defaults")),
        Command::Gpu { mode, nouveau, force } => {
            let driver = if nouveau { "open" } else { "auto" };
            p.set_gpu_mode(&mode, driver, force).await.map(|_| {
                let driver_note = if nouveau { " (nouveau)" } else { "" };
                if force {
                    println!("gpu mode applied immediately: {mode}{driver_note}");
                } else {
                    println!("gpu mode queued: {mode}{driver_note} (applies on next logout/boot; use -f to apply now)");
                }
            })
        }
        Command::GpuCancel => p.clear_gpu_pending().await.map(|was_queued| {
            if was_queued {
                println!("queued GPU switch cancelled");
            } else {
                println!("no GPU switch was queued");
            }
        }),
        Command::Status { json } => match p.get_status().await {
            Ok(status_json) => {
                if json {
                    println!("{status_json}");
                } else {
                    print_status(&status_json);
                }
                Ok(())
            }
            Err(e) => Err(e),
        },
        Command::GpuList => match p.list_gpu_devices().await {
            Ok(json) => {
                println!("{json}");
                Ok(())
            }
            Err(e) => Err(e),
        },
        Command::Conflicts => match p.get_conflicts().await {
            Ok(list) => {
                if list.is_empty() {
                    println!("no conflicting services active");
                } else {
                    for s in list {
                        println!("ACTIVE (conflict): {s}");
                    }
                }
                Ok(())
            }
            Err(e) => Err(e),
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn print_status(json: &str) {
    match serde_json::from_str::<DaemonStatus>(json) {
        Ok(s) => {
            println!("Profile:        {}", s.active_profile.map(|p| p.to_string()).unwrap_or("(none)".into()));
            println!("Power backend:  {}", s.power_backend);
            println!(
                "TDP:            PL1={} PL2={}",
                s.tdp_pl1_watts.map(|w| format!("{w}W")).unwrap_or("?".into()),
                s.tdp_pl2_watts.map(|w| format!("{w}W")).unwrap_or("?".into())
            );
            println!("GPU mode:       {}", s.gpu_mode.map(|m| m.to_string()).unwrap_or("unknown".into()));
            println!("GPU power:      {}", s.gpu_power_state);
            if let Some(pending) = s.gpu_pending {
                println!("GPU pending:    {pending}");
            }
            if let Some(err) = s.gpu_last_auto_apply_error {
                println!("GPU auto-apply FAILED last time: {err}");
            }
            if s.tlp_service_active {
                println!("TLP:            active");
            }
            if !s.conflicting_services.is_empty() {
                println!("Conflicts:      {}", s.conflicting_services.join(", "));
            }
        }
        Err(e) => println!("(failed to parse status JSON: {e})\n{json}"),
    }
}
