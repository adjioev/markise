//! Generic HTML, CSS and JS to an image in a helper process (ADR 0023): the protocol and the
//! client. It knows no web engine, no package and no Markdown.

pub mod cache;
pub mod client;
pub mod proto;

pub use cache::ShotCache;
pub use client::{
    Availability, Client, Job, JobSource, Limits, Outcome, Prepared, ProcessSpawner, Shot, Spawned, Spawner,
};
