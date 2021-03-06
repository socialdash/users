//! Users is a microservice responsible for authentication and managing user profiles.
//! The layered structure of the app is
//!
//! `Application -> Controller -> Service -> Repo + HttpClient`
//!
//! Each layer can only face exceptions in its base layers and can only expose its own errors.
//! E.g. `Service` layer will only deal with `Repo` and `HttpClient` errors and will only return
//! `ServiceError`. That way Controller will only have to deal with ServiceError, but not with `Repo`
//! or `HttpClient` repo.

#![allow(proc_macro_derive_resolution_fallback)]
extern crate base64;
extern crate chrono;
extern crate config as config_crate;
#[macro_use]
extern crate diesel;
#[macro_use]
extern crate failure;
extern crate futures;
extern crate futures_cpupool;
extern crate hyper;
extern crate hyper_tls;
extern crate jsonwebtoken;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate r2d2;
extern crate r2d2_redis;
extern crate rand;
extern crate regex;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate sha3;
extern crate tokio_core;
extern crate tokio_signal;
extern crate uuid;
extern crate validator;
#[macro_use]
extern crate validator_derive;
#[macro_use]
extern crate sentry;
extern crate stq_cache;
extern crate stq_http;
extern crate stq_logging;
extern crate stq_router;
extern crate stq_static_resources;
extern crate stq_types;

#[macro_use]
pub mod macros;
pub mod config;
pub mod controller;
pub mod errors;
pub mod models;
pub mod repos;
#[rustfmt::skip]
pub mod schema;
pub mod sentry_integration;
pub mod services;

use std::fs::File;
use std::io::prelude::*;
use std::process;
use std::sync::Arc;
use std::time::Duration;

use diesel::pg::PgConnection;
use diesel::r2d2::ConnectionManager;
use futures::{Future, Stream};
use futures_cpupool::CpuPool;
use hyper::server::Http;
use r2d2_redis::RedisConnectionManager;
use stq_cache::cache::{redis::RedisCache, Cache, NullCache, TypedCache};
use stq_http::controller::Application;
use tokio_core::reactor::Core;

use config::Config;
use controller::context::StaticContext;
use errors::Error;
use repos::acl::RolesCacheImpl;
use repos::repo_factory::ReposFactoryImpl;

/// Starts new web service from provided `Config`
pub fn start_server(config: Config) {
    // Prepare reactor
    let mut core = Core::new().expect("Unexpected error creating event loop core");
    let handle = Arc::new(core.handle());
    let client = stq_http::client::Client::new(&config.to_http_config(), &handle);
    let client_handle = client.handle();
    let client_stream = client.stream();
    handle.spawn(client_stream.for_each(|_| Ok(())));

    // Prepare server
    let thread_count = config.server.thread_count;

    // Prepare server
    let address = {
        format!("{}:{}", config.server.host, config.server.port)
            .parse()
            .expect("Could not parse address")
    };

    // Prepare database pool
    let database_url: String = config.server.database.parse().expect("Database URL must be set in configuration");
    let db_manager = ConnectionManager::<PgConnection>::new(database_url);
    let db_pool = r2d2::Pool::builder()
        .build(db_manager)
        .expect("Failed to create DB connection pool");

    // Prepare CPU pool
    let cpu_pool = CpuPool::new(thread_count);

    // Prepare cache
    let roles_cache = match &config.server.redis {
        Some(redis_url) => {
            // Prepare Redis pool
            let redis_url: String = redis_url.parse().expect("Redis URL must be set in configuration");
            let redis_manager = RedisConnectionManager::new(redis_url.as_ref()).expect("Failed to create Redis connection manager");
            let redis_pool = r2d2::Pool::builder()
                .build(redis_manager)
                .expect("Failed to create Redis connection pool");

            let ttl = Duration::from_secs(config.server.cache_ttl_sec);

            let roles_cache_backend = Box::new(TypedCache::new(
                RedisCache::new(redis_pool.clone(), "roles".to_string()).with_ttl(ttl),
            )) as Box<dyn Cache<_, Error = _> + Send + Sync>;

            RolesCacheImpl::new(roles_cache_backend)
        }
        None => RolesCacheImpl::new(Box::new(NullCache::new()) as Box<_>),
    };

    let repo_factory = ReposFactoryImpl::new(roles_cache);

    debug!("Reading private key file {}", &config.jwt.secret_key_path);
    let mut f = File::open(config.jwt.secret_key_path.clone()).unwrap();
    let mut jwt_private_key: Vec<u8> = Vec::new();
    f.read_to_end(&mut jwt_private_key).unwrap();

    let context = StaticContext::new(db_pool, cpu_pool, client_handle, Arc::new(config), repo_factory, jwt_private_key);

    let serve = Http::new()
        .serve_addr_handle(&address, &handle, move || {
            // Prepare application
            let controller = controller::ControllerImpl::new(context.clone());
            let app = Application::<Error>::new(controller);

            Ok(app)
        })
        .unwrap_or_else(|why| {
            error!("Http Server Initialization Error: {}", why);
            process::exit(1);
        });

    let handle_arc2 = handle.clone();
    handle.spawn(
        serve
            .for_each(move |conn| {
                handle_arc2.spawn(conn.map(|_| ()).map_err(|why| error!("Server Error: {:?}", why)));
                Ok(())
            })
            .map_err(|_| ()),
    );

    info!("Listening on http://{}, threads: {}", address, thread_count);

    core.run(tokio_signal::ctrl_c().flatten_stream().take(1u64).for_each(|()| {
        info!("Ctrl+C received. Exit");
        Ok(())
    }))
    .unwrap();
}
