use std::error::Error;
use std::net::SocketAddr;

use hoglet::application::{Application, ApplicationConfig};

const HELP: &str = "\
hoglet — PostHog-compatible product analytics. One binary.

USAGE:
    hoglet                  Start the server
    hoglet import posthog   Copy a PostHog project's history into Hoglet
                            (see `hoglet import posthog --help`)
    hoglet reconcile posthog
                            Compare Hoglet's numbers with PostHog's
                            (see `hoglet reconcile posthog --help`)
    hoglet healthcheck      Exit 0 if the local server is ready (for Docker HEALTHCHECK)
    hoglet --version        Print the version

ENVIRONMENT:
    HOGLET_ADDR                 Listen address            (default 127.0.0.1:8000)
    HOGLET_DATA                 Data directory            (default ./hoglet-data)
    HOGLET_RETENTION_DAYS       Delete events older than N days (default: keep all)
    HOGLET_MAX_EVENTS_PER_SEC   Per-project capture limit (default 10000)
    HOGLET_COOKIELESS_SALT      Enable cookieless device ids with this secret salt
    HOGLET_NO_UA_PARSE          Set to disable user-agent enrichment
    HOGLET_DEMO                 Set to 1 on a fresh data directory to create a demo
                                account (demo@hoglet.dev / hoglet-demo-1) with 90 days
                                of realistic data
    RUST_LOG                    Log filter                (default hoglet=info)

Point any PostHog SDK at this server: api_host = \"http://<HOGLET_ADDR>\".
";

const DEMO_EMAIL: &str = "demo@hoglet.dev";
const DEMO_PASSWORD: &str = "hoglet-demo-1";

fn env_number<T: std::str::FromStr>(name: &str) -> Result<Option<T>, Box<dyn Error>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| format!("{name} must be a number, got {value:?}").into()),
        Err(_) => Ok(None),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            return Ok(());
        }
        Some("-V" | "--version" | "version") => {
            println!("hoglet {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("import") => return run_import().await,
        Some("reconcile") => return run_reconcile().await,
        Some("healthcheck") => return healthcheck().await,
        Some("serve") | None => {}
        Some(other) => return Err(format!("unknown command {other:?}; see `hoglet --help`").into()),
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hoglet=info".into()),
        )
        .init();

    let data_dir = std::path::PathBuf::from(
        std::env::var("HOGLET_DATA").unwrap_or_else(|_| "hoglet-data".into()),
    );
    let addr: SocketAddr = std::env::var("HOGLET_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8000".into())
        .parse()
        .map_err(|error| format!("HOGLET_ADDR must be a socket address: {error}"))?;
    let mut config = ApplicationConfig::new(&data_dir);
    if let Some(limit) = env_number("HOGLET_MAX_EVENTS_PER_SEC")? {
        config.max_events_per_second = limit;
    }
    config.retention_days = env_number("HOGLET_RETENTION_DAYS")?;
    config.enrichment = hoglet::enrichment::EnrichmentConfig::from_env();

    let started = std::time::Instant::now();
    let application = Application::prepare(config).await?;
    let demo = if std::env::var("HOGLET_DEMO").is_ok_and(|value| value == "1") {
        seed_demo(&application).await?
    } else {
        false
    };
    let listener = tokio::net::TcpListener::bind(addr).await?;
    application.mark_ready();

    let shown = if addr.ip().is_unspecified() {
        format!("http://localhost:{}", addr.port())
    } else {
        format!("http://{addr}")
    };
    eprintln!();
    eprintln!("  hoglet {} ready in {} ms", env!("CARGO_PKG_VERSION"), started.elapsed().as_millis());
    eprintln!();
    eprintln!("  dashboard   {shown}");
    eprintln!("  api_host    {shown}   (point any PostHog SDK here)");
    eprintln!("  data        {}", data_dir.display());
    if demo {
        eprintln!("  demo login  {DEMO_EMAIL} / {DEMO_PASSWORD}");
    }
    eprintln!();
    tracing::info!(%addr, "hoglet listening");

    let shutdown_readiness = application.readiness();
    let server_result = axum::serve(listener, application.router())
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_readiness.mark_not_ready();
            tracing::info!("shutting down: draining requests and publishing the WAL");
        })
        .await;
    let shutdown_result = application.shutdown().await;
    server_result?;
    shutdown_result?;
    Ok(())
}

/// `GET /ready` on this server's own address; exit status 0 when it answers 200.
async fn healthcheck() -> Result<(), Box<dyn Error>> {
    let addr = std::env::var("HOGLET_ADDR").unwrap_or_else(|_| "127.0.0.1:8000".into());
    let addr: SocketAddr = addr
        .parse()
        .map_err(|error| format!("HOGLET_ADDR must be a socket address: {error}"))?;
    // A wildcard listen address is reachable through loopback.
    let host = if addr.ip().is_unspecified() {
        format!("127.0.0.1:{}", addr.port())
    } else {
        addr.to_string()
    };
    let url = format!("http://{host}/ready");
    let status = tokio::task::spawn_blocking(move || {
        ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .get(&url)
            .call()
            .map(|response| response.status())
            .map_err(|error| error.to_string())
    })
    .await?;
    match status {
        Ok(200) => Ok(()),
        Ok(code) => Err(format!("not ready: HTTP {code}").into()),
        Err(error) => Err(format!("not ready: {error}").into()),
    }
}

async fn run_import() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match args.first().map(String::as_str) {
        Some("posthog") => {}
        _ => return Err("usage: hoglet import posthog --help".into()),
    }
    if args.len() == 1 || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{}", hoglet::import::HELP);
        return Ok(());
    }
    let config = hoglet::import::parse_args(&args[1..])?;
    eprintln!(
        "importing PostHog project {} from {} into {}",
        config.posthog_project_id, config.posthog_host, config.hoglet_host
    );
    let report = tokio::task::spawn_blocking(move || {
        hoglet::import::Importer::new(config, Box::new(|line| eprintln!("  {line}"))).run()
    })
    .await??;
    eprintln!(
        "done: {} events, {} persons, {} flags",
        report.events, report.persons, report.flags
    );
    Ok(())
}

async fn run_reconcile() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match args.first().map(String::as_str) {
        Some("posthog") => {}
        _ => return Err("usage: hoglet reconcile posthog --help".into()),
    }
    if args.len() == 1 || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{}", hoglet::reconcile::HELP);
        return Ok(());
    }
    let config = hoglet::reconcile::parse_args(&args[1..])?;
    let tolerance = config.tolerance_percent;
    let rows = tokio::task::spawn_blocking(move || hoglet::reconcile::run(&config)).await??;
    let (table, all_match) = hoglet::reconcile::render(&rows, tolerance);
    print!("{table}");
    if !all_match {
        std::process::exit(1);
    }
    Ok(())
}

/// Create the demo account and data on a fresh install. Returns whether the
/// demo account exists afterwards.
async fn seed_demo(application: &Application) -> Result<bool, Box<dyn Error>> {
    let access = application.access();
    if !access.setup_required().await? {
        tracing::info!("HOGLET_DEMO ignored: this data directory already has an account");
        return Ok(false);
    }
    let setup = access
        .setup(hoglet::control::SetupRequest {
            email: DEMO_EMAIL.to_owned(),
            password: DEMO_PASSWORD.to_owned(),
            organization_name: "Demo".to_owned(),
            project_name: "Notably (demo)".to_owned(),
            existing_project_token: None,
        })
        .await?;
    let project = setup
        .workspace
        .organizations
        .first()
        .and_then(|organization| organization.projects.first())
        .ok_or("demo setup created no project")?;
    tracing::info!("generating demo data");
    let events = hoglet::routes::demo::seed_project(
        application.sink().as_ref(),
        &project.id,
        &project.token,
    )
    .await
    .map_err(|()| "demo data could not be stored")?;
    tracing::info!(events, "demo data stored");
    Ok(true)
}

async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
