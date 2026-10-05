//! Safe production application composition.
//!
//! `Application::prepare` is the startup-policy seam: inspection always comes
//! before mutation, every authoritative dependency is required, and only the
//! production trust classes are mounted.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use tower_http::cors::CorsLayer;

use crate::capture::{CaptureAuthorizer, CaptureState, ProjectAccessCaptureAuthorizer};
use crate::control::{AccessError, ProjectAccess, ProjectAccessRuntime};
use crate::control_resources::{ControlResourceError, ControlResources};
use crate::lake::{Lake, LakeError};
use crate::persons::{PersonStore, PersonStoreError};
use crate::projection_catalog::{ProjectionCatalog, ProjectionCatalogError};
use crate::query::QueryEngine;
use crate::routes::health::Readiness;
use crate::sink::{
    DurablePipelineError, DurableWalRuntime, DurableWalSink, PipelineConfig, PipelineStats,
};
use crate::storage_bootstrap::{
    StorageBootstrapError, StorageDisposition, StoragePaths, bootstrap_storage, inspect_storage,
};

#[derive(Debug, Clone)]
pub struct ApplicationConfig {
    pub data_dir: PathBuf,
    pub max_events_per_second: u32,
    /// Drop event files older than this many days.
    pub retention_days: Option<u32>,
    pub enrichment: crate::enrichment::EnrichmentConfig,
    pub security: crate::security::SecurityConfig,
}

impl ApplicationConfig {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            max_events_per_second: crate::ratelimit::DEFAULT_MAX_PER_SEC,
            retention_days: None,
            enrichment: crate::enrichment::EnrichmentConfig::default(),
            security: crate::security::SecurityConfig::default(),
        }
    }
}

#[derive(Debug)]
pub enum ApplicationError {
    MigrationIncomplete {
        data_dir: PathBuf,
    },
    Storage(StorageBootstrapError),
    Access(AccessError),
    Resources(ControlResourceError),
    ProjectionCatalog(ProjectionCatalogError),
    Lake(LakeError),
    Persons(PersonStoreError),
    Query(String),
    Wal(crate::pipeline::wal::WalError),
    Pipeline(DurablePipelineError),
    LocalStore(String),
    Flags(String),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MigrationIncomplete { data_dir } => write!(
                formatter,
                "incomplete storage found in {}: one of control.db/projections.db is missing",
                data_dir.display()
            ),
            Self::Storage(error) => write!(formatter, "storage bootstrap failed: {error}"),
            Self::Access(error) => write!(formatter, "project access failed: {error}"),
            Self::Resources(error) => write!(formatter, "control resources failed: {error}"),
            Self::ProjectionCatalog(error) => {
                write!(formatter, "projection catalog failed: {error}")
            }
            Self::Lake(error) => write!(formatter, "event lake failed: {error}"),
            Self::Persons(error) => write!(formatter, "person store failed: {error}"),
            Self::Query(error) => write!(formatter, "query engine failed: {error}"),
            Self::Wal(error) => write!(formatter, "durable capture WAL failed: {error}"),
            Self::Pipeline(error) => write!(formatter, "durable pipeline failed: {error}"),
            Self::LocalStore(error) => write!(formatter, "ephemeral wire state failed: {error}"),
            Self::Flags(error) => write!(formatter, "feature flags failed: {error}"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ApplicationError {}

impl From<StorageBootstrapError> for ApplicationError {
    fn from(error: StorageBootstrapError) -> Self {
        Self::Storage(error)
    }
}
impl From<AccessError> for ApplicationError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}
impl From<ControlResourceError> for ApplicationError {
    fn from(error: ControlResourceError) -> Self {
        Self::Resources(error)
    }
}
impl From<ProjectionCatalogError> for ApplicationError {
    fn from(error: ProjectionCatalogError) -> Self {
        Self::ProjectionCatalog(error)
    }
}
impl From<LakeError> for ApplicationError {
    fn from(error: LakeError) -> Self {
        Self::Lake(error)
    }
}
impl From<PersonStoreError> for ApplicationError {
    fn from(error: PersonStoreError) -> Self {
        Self::Persons(error)
    }
}
impl From<crate::pipeline::wal::WalError> for ApplicationError {
    fn from(error: crate::pipeline::wal::WalError) -> Self {
        Self::Wal(error)
    }
}
impl From<DurablePipelineError> for ApplicationError {
    fn from(error: DurablePipelineError) -> Self {
        Self::Pipeline(error)
    }
}

pub struct Application {
    router: Router,
    readiness: Readiness,
    wal_runtime: DurableWalRuntime,
    control_runtime: ProjectAccessRuntime,
    lake: Arc<Lake>,
    persons: Arc<PersonStore>,
    access: Arc<ProjectAccess>,
    sink: Arc<dyn crate::sink::EventSink>,
    forwarder: Arc<crate::forward::Forwarder>,
}

impl fmt::Debug for Application {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Application")
            .finish_non_exhaustive()
    }
}

impl Application {
    pub async fn prepare(config: ApplicationConfig) -> Result<Self, ApplicationError> {
        restrict_data_directory(&config.data_dir)?;
        let paths = StoragePaths::new(&config.data_dir);
        match inspect_storage(&paths)? {
            StorageDisposition::Fresh | StorageDisposition::ReadyV2(_) => {
                bootstrap_storage(paths.control(), paths.projections())?;
            }
            StorageDisposition::MigrationIncomplete if recoverable_fresh_bootstrap(&paths) => {
                // Fresh-pair creation renames projections first. If the
                // process died before the final control rename, the staged
                // control database is validated against it and completed.
                bootstrap_storage(paths.control(), paths.projections())?;
            }
            StorageDisposition::MigrationIncomplete => {
                return Err(ApplicationError::MigrationIncomplete {
                    data_dir: config.data_dir,
                });
            }
        }

        let (access, control_runtime) = ProjectAccess::open(paths.control())?;
        let access = Arc::new(access);
        let resources = Arc::new(ControlResources::open(&paths.control())?);
        let lake = Arc::new(Lake::open(&paths.projections(), &config.data_dir.join("events"))?);
        let persons = Arc::new(PersonStore::open(&paths.projections())?);
        let (durable_sink, wal_runtime, recovery) = DurableWalSink::open(
            PipelineConfig {
                wal_dir: config.data_dir.join("wal"),
                tmp_dir: config.data_dir.join("tmp"),
                retention_days: config.retention_days,
            },
            lake.clone(),
        )?;
        if recovery.truncated_tail {
            tracing::warn!("recovered a torn tail from the capture WAL");
        }
        let engine = Arc::new(
            QueryEngine::new(
                lake.clone() as Arc<dyn crate::source::EventSource>,
                persons.clone(),
                crate::query::EngineConfig {
                    temp_directory: Some(config.data_dir.join("tmp").join("query")),
                    ..crate::query::EngineConfig::default()
                },
            )
            .map_err(|error| ApplicationError::Query(error.to_string()))?,
        );
        let projection_catalog = Arc::new(ProjectionCatalog::open(&paths.projections())?);
        let event_source: Arc<dyn crate::source::EventSource> = lake.clone();
        let explorer = Arc::new(
            crate::explore::Explorer::with_spill_dir(
                event_source,
                persons.clone(),
                Some(config.data_dir.join("tmp").join("explore")),
            )
                .map_err(|error| ApplicationError::Query(error.to_string()))?,
        );

        let flag_store = Arc::new(
            crate::flags::FlagStore::open(&paths.control())
                .map_err(|error| ApplicationError::Flags(error.to_string()))?,
        );
        let metrics = Arc::new(crate::metrics::Metrics::new(
            chrono::Utc::now().timestamp().max(0) as u64,
        ));
        let authorizer: Arc<dyn CaptureAuthorizer> = Arc::new(
            ProjectAccessCaptureAuthorizer::new(access.as_ref().clone()),
        );
        let sink: Arc<dyn crate::sink::EventSink> = durable_sink;
        let forwarder = crate::forward::Forwarder::open_with_policy(
            &paths.control(),
            config.security.allow_private_forwarding,
        )
            .map_err(|error| ApplicationError::LocalStore(format!("forwarding: {error}")))?;
        let capture = CaptureState {
            sink: sink.clone(),
            authorizer: authorizer.clone(),
            limiter: Arc::new(crate::ratelimit::RateLimiter::new(
                config.max_events_per_second,
            )),
            metrics: metrics.clone(),
            enricher: Arc::new(crate::enrichment::Enricher::new(config.enrichment.clone())),
            forwarder: Some(forwarder.clone()),
        };
        let readiness = Readiness::new();
        let wire = crate::capture::router(capture)
            .merge(crate::routes::config::wire_router(authorizer.clone()))
            .merge(crate::routes::flags::wire_router(
                flag_store.clone(),
                persons.clone(),
                authorizer,
            ))
            .layer(CorsLayer::very_permissive());
        let public = crate::routes::dashboard::router()
            .merge(crate::routes::health::router(readiness.clone()))
            .merge(crate::routes::metrics::router_with_token(
                metrics,
                config.security.metrics_token.clone(),
            ))
            .merge(crate::routes::docs::router());
        // Sign-in and invite acceptance share one failure throttle.
        let throttle = Arc::new(crate::security::LoginThrottle::default());
        let dashboard = crate::routes::workspace::router_with_throttle(
            access.clone(),
            config.security.clone(),
            throttle.clone(),
        )
            .merge(crate::routes::members::router(
                access.clone(),
                config.security.clone(),
                throttle,
            ))
            .merge(crate::routes::project::router(access.clone(), engine))
            .merge(crate::routes::catalog_v2::router(
                access.clone(),
                projection_catalog,
            ))
            .merge(crate::routes::status::router(
                access.clone(),
                lake.clone(),
                wal_runtime.stats(),
            ))
            .merge(crate::routes::demo::router(access.clone(), sink.clone()))
            .merge(crate::routes::forwarding::router(access.clone(), forwarder.clone()))
            .merge(crate::routes::erasure::router(
                access.clone(),
                wal_runtime.eraser(),
            ))
            .merge(crate::routes::flags::api_router(
                access.clone(),
                flag_store,
                persons.clone(),
            ))
            .merge(crate::routes::persons::router(
                access.clone(),
                explorer.clone(),
            ))
            .merge(crate::routes::events::router(
                access.clone(),
                explorer.clone(),
            ))
            .merge(crate::routes::web::router(access.clone(), explorer))
            .merge(crate::routes::resources::router(access.clone(), resources))
            // Cookie-authenticated mutations must come from this origin.
            .layer(axum::middleware::from_fn(crate::security::csrf_guard));

        Ok(Self {
            router: wire
                .merge(public)
                .merge(dashboard)
                .layer(axum::middleware::from_fn(crate::security::security_headers)),
            readiness,
            wal_runtime,
            control_runtime,
            lake,
            persons,
            access,
            sink,
            forwarder,
        })
    }

    /// The durable capture sink: what `/e` and `/batch` append to.
    pub fn sink(&self) -> Arc<dyn crate::sink::EventSink> {
        self.sink.clone()
    }

    pub fn lake(&self) -> Arc<Lake> {
        self.lake.clone()
    }

    pub fn persons(&self) -> Arc<PersonStore> {
        self.persons.clone()
    }

    pub fn access(&self) -> Arc<ProjectAccess> {
        self.access.clone()
    }

    pub fn pipeline_stats(&self) -> Arc<PipelineStats> {
        self.wal_runtime.stats()
    }

    pub fn router(&self) -> Router {
        self.router.clone()
    }
    pub fn mark_ready(&self) {
        self.readiness.mark_ready();
    }
    pub fn mark_not_ready(&self) {
        self.readiness.mark_not_ready();
    }
    pub fn readiness(&self) -> Readiness {
        self.readiness.clone()
    }

    pub async fn shutdown(self) -> Result<(), ApplicationError> {
        self.readiness.mark_not_ready();
        self.forwarder.stop();
        drop(self.router);
        let wal_result = self.wal_runtime.shutdown().await;
        self.control_runtime.close();
        wal_result?;
        Ok(())
    }
}

/// The data directory holds every event and credential: owner-only, even when
/// the operator created it (a Docker volume, systemd `StateDirectory`) with a
/// looser mode.
fn restrict_data_directory(data_dir: &Path) -> Result<(), ApplicationError> {
    // A missing directory is created owner-only by storage bootstrap.
    if !data_dir.is_dir() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700)).map_err(
            |source| ApplicationError::Io {
                path: data_dir.to_path_buf(),
                source,
            },
        )?;
    }
    Ok(())
}

fn recoverable_fresh_bootstrap(paths: &StoragePaths) -> bool {
    !paths.control().exists()
        && paths.projections().is_file()
        && paths.control_migrating().is_file()
        && !paths.projections_migrating().exists()
}


