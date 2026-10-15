// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! CrowdWatch: an aggregate movement observatory with a SkeleTrace import adapter.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod cli;
pub mod config;
pub mod csvread;
pub mod demo;
pub mod error;
pub mod export;
pub mod http;
pub mod json;
pub mod model;
pub mod pyfmt;
pub mod pytime;
pub mod report;
pub mod sources;
pub mod store;
mod unicode_tables;
pub mod urlparse;
pub mod xml;
mod xml_encodings;
