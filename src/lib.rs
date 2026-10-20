//! Zetlyn: a URL in, its changes out.
//!
//! Everything the product does lives here, in one application: accounts, watches, reading
//! pages, finding a marked place again, comparing, the mails, the free day and the subscription
//! (`plan.rs`), the site with its legal pages, the API and MCP. The service at zetlyn.com is this
//! application on our servers, an instance per server, with one SQLite file each.

pub mod alert;
pub mod billing;
pub mod api;
pub mod anchor;

pub mod check;
pub mod cli;
pub mod crypto;
pub mod docs;
pub mod fetch;
pub mod life;
pub mod mail;
pub mod outbound;
pub mod oauth;
pub mod page;
pub mod plan;
pub mod render;
pub mod serve;
pub mod store;
pub mod tell;
pub mod text;
pub mod when;
