//! Marketplace system for plugin discovery and management.
//!
//! Provides marketplace management, official marketplace definitions,
//! marketplace helpers, startup checks, and input parsing.

pub mod helpers;
pub mod input_parser;
pub mod manager;
pub mod official;
pub mod startup_check;

pub use helpers::{
    create_plugin_id, fetch_marketplace_index, format_failure_details,
    get_marketplace_source_display, parse_marketplace_index, resolve_marketplace_source,
};
pub use input_parser::{MarketplaceInput, parse_marketplace_input};
pub use manager::{MarketplaceEntry, MarketplaceIndex, MarketplaceManager};
pub use official::{
    OFFICIAL_MARKETPLACE_NAME, OFFICIAL_MARKETPLACE_SOURCE, get_official_marketplaces,
    is_official_marketplace,
};
pub use startup_check::{
    MarketplaceSkipReason, MarketplaceStartupCheckResult, perform_marketplace_startup_checks,
};
