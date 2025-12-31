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

pub mod acme;
pub mod admin;
pub mod config;
pub mod docker;
pub mod error;
pub mod pool;
pub mod process;
pub mod proxy;
