use anyhow::Context;
use clap::{Parser, builder::ValueParser, crate_version};
use std::collections::{HashMap, HashSet};

fn parse_extensions(s: &str) -> anyhow::Result<(String, String)> {
    s.split_once('=')
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .with_context(|| format!("Invalid KEY=VALUE format: {}", s))
}

#[derive(Parser, Debug)]
#[command(
    author,
    version = crate_version!(),
    about,
    long_about = None
)]
pub(crate) struct Args {
    /// Additional file extension mappings, e.g. -E c++=cpp -E cxx=cpp
    #[arg(
        short = 'E',
        long = "extension",
        value_name = "KEY=VALUE",
        action = clap::ArgAction::Append,
        value_parser = ValueParser::new(parse_extensions),
    )]
    extensions: Vec<(String, String)>,
}

impl Args {
    pub(crate) fn extensions(&self) -> HashMap<String, String> {
        self.extensions
            .iter()
            .map(|(key, val)| (key.clone(), val.clone()))
            .collect()
    }

    pub(crate) fn validate(&self, supported_extensions: HashSet<String>) -> anyhow::Result<()> {
        for (key, val) in &self.extensions {
            if !supported_extensions.contains(val) {
                anyhow::bail!("Unsupported extension mapping: {}={}", key, val);
            }
        }

        Ok(())
    }
}
