#![feature(btree_drain_filter)]
#![feature(doc_cfg)]
#![feature(duration_zero)]
#![feature(fn_traits)]
#![feature(pattern)]
#![feature(unboxed_closures)]
#![feature(untagged_unions)]

#[macro_use] extern crate log;
#[macro_use] extern crate derive_more;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate gen_settings;

pub mod archive;
mod arena;
pub mod derivation;
pub mod eval;
mod fetch;
pub mod goal;
pub mod hash;
pub mod path;
pub mod path_info;
mod prelude;
pub mod settings;
mod sqlite;
pub mod store;
pub mod syntax;
pub mod util;

pub use settings::Settings;
pub use store::Store;

pub fn settings() -> &'static Settings {
  Settings::get()
}
