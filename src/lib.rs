//! Spawngate - A reverse proxy that spawns backends on demand
//!
//! This library provides a serverless-style reverse proxy that:
//! - Routes HTTP traffic based on Host header to configured backends
//! - Spawns backend processes on-demand when traffic arrives
//! - Supports both local processes and Docker containers as backends
//! - Monitors backend health via polling and callback mechanisms
//! - Automatically shuts down idle backends after a configurable timeout
//! - Uses connection pooling for efficient backend communication
//! - Supports automatic TLS via ACME/Let's Encrypt
//! - Provides add-on services (PostgreSQL, Redis, S3-compatible storage)
//! - Builds apps from source using Cloud Native Buildpacks
//! - Supports git push deployment workflow

pub mod acme;
pub mod addons;
pub mod alerting;
pub mod admin;
pub mod api;
pub mod auth;
pub mod builder;
pub mod buildpacks;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod docker;
pub mod domains;
pub mod instance;
pub mod error;
pub mod git;
pub mod healthcheck;
pub mod loadbalancer;
pub mod notifications;
pub mod pool;
pub mod process;
pub mod proxy;
pub mod secrets;
pub mod webhooks;
