//! JSON Schemas of tool arguments (ADR-0010).
//!
//! Generated from the same types [`crate::ToolRuntime::invoke`] deserializes,
//! so what an AI model is told matches what the runtime accepts.

use crate::CATALOG;
use crate::{
    filesystem, git_tools, github_tools, package, process, project, shell, terminal, web, Empty,
    OpenArgs,
};
use orchestrator_core::ToolDefinition;
use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde_json::Value;
use std::sync::OnceLock;

/// Draft-7 schema with every subschema inlined (no `$ref`) and no
/// `$schema`/`title`, the most portable form across AI APIs.
fn schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft07()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        })
        .into_generator();
    let mut value = generator.into_root_schema_for::<T>().to_value();
    if let Some(object) = value.as_object_mut() {
        object.remove("title");
        object.remove("description");
        // Tools without arguments still get an explicit empty object shape;
        // some APIs require `properties`.
        object
            .entry("properties")
            .or_insert_with(|| Value::Object(Default::default()));
    }
    value
}

/// Schema of the arguments of `tool`, or `None` for an unknown tool.
pub fn parameters(tool: &str) -> Option<Value> {
    let schema = match tool {
        "filesystem.list" => schema::<filesystem::ListArgs>(),
        "filesystem.read" => schema::<filesystem::ReadArgs>(),
        "filesystem.write" => schema::<filesystem::WriteArgs>(),
        "filesystem.move" => schema::<filesystem::MoveArgs>(),
        "filesystem.delete" => schema::<filesystem::DeleteArgs>(),
        "shell.execute" => schema::<shell::ExecuteArgs>(),
        "web.fetch" => schema::<web::FetchArgs>(),
        "http.request" => schema::<web::RequestArgs>(),
        "secrets.list" => schema::<Empty>(),
        "terminal.create" => schema::<terminal::CreateArgs>(),
        "terminal.write" => schema::<terminal::WriteArgs>(),
        "terminal.read" => schema::<terminal::ReadArgs>(),
        "terminal.close" => schema::<terminal::IdArgs>(),
        "process.start" => schema::<process::StartArgs>(),
        "process.stop" => schema::<process::StopArgs>(),
        "process.read" => schema::<process::ReadArgs>(),
        "project.discover" => schema::<project::DiscoverArgs>(),
        "project.profile" => schema::<project::PathArgs>(),
        "project.open" => schema::<OpenArgs>(),
        "git.status" => schema::<git_tools::StatusArgs>(),
        "git.diff" => schema::<git_tools::DiffArgs>(),
        "git.log" => schema::<git_tools::LogArgs>(),
        "git.branch" => schema::<git_tools::BranchArgs>(),
        "git.checkout" => schema::<git_tools::CheckoutArgs>(),
        "git.add" => schema::<git_tools::AddArgs>(),
        "git.commit" => schema::<git_tools::CommitArgs>(),
        "git.pull" => schema::<git_tools::PullArgs>(),
        "git.push" => schema::<git_tools::PushArgs>(),
        "git.stash" => schema::<git_tools::StashArgs>(),
        "git.reset" => schema::<git_tools::ResetArgs>(),
        "git.remotes" => schema::<github_tools::RemotesArgs>(),
        "git.fetch" => schema::<github_tools::FetchArgs>(),
        "github.status" => schema::<github_tools::StatusArgs>(),
        "github.pr.list" => schema::<github_tools::PrListArgs>(),
        "github.pr.get" | "github.issue.get" => schema::<github_tools::NumberArgs>(),
        "github.checks" => schema::<github_tools::ChecksArgs>(),
        "github.pr.create" => schema::<github_tools::PrCreateArgs>(),
        "github.pr.comment" | "github.issue.comment" => schema::<github_tools::CommentArgs>(),
        "github.pr.merge" => schema::<github_tools::PrMergeArgs>(),
        "github.issue.list" => schema::<github_tools::IssueListArgs>(),
        "github.issue.create" => schema::<github_tools::IssueCreateArgs>(),
        "package.install" => schema::<package::InstallArgs>(),
        "package.run" => schema::<package::RunArgs>(),
        "shell.list" | "terminal.list" | "process.list" | "runtime.node" | "runtime.python"
        | "runtime.docker" => schema::<Empty>(),
        _ => return None,
    };
    Some(schema)
}

/// Every catalog tool with its argument schema (computed once).
pub fn definitions() -> &'static [ToolDefinition] {
    static DEFINITIONS: OnceLock<Vec<ToolDefinition>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        CATALOG
            .iter()
            .map(|spec| ToolDefinition {
                name: spec.name.to_owned(),
                group: spec.group.to_owned(),
                description: spec.description.to_owned(),
                read_only: spec.read_only,
                parameters: parameters(spec.name)
                    .unwrap_or_else(|| panic!("tool {} has no argument schema", spec.name)),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_tool_has_an_object_schema() {
        for spec in CATALOG {
            let schema = parameters(spec.name).unwrap_or_else(|| panic!("{}", spec.name));
            assert_eq!(schema["type"], "object", "{}", spec.name);
            let text = schema.to_string();
            assert!(!text.contains("$ref"), "{} has $ref", spec.name);
            assert!(!text.contains("$schema"), "{}", spec.name);
        }
        assert_eq!(definitions().len(), CATALOG.len());
    }

    #[test]
    #[ignore = "prints schemas for inspection"]
    fn print_schemas() {
        for d in definitions() {
            println!("{} {}", d.name, d.parameters);
        }
    }

    #[test]
    fn schemas_follow_the_wire_format() {
        let read = parameters("filesystem.read").unwrap();
        assert_eq!(read["required"], serde_json::json!(["path"]));
        assert!(read["properties"]["maxBytes"].is_object(), "{read}");
        assert_eq!(read["additionalProperties"], false);
        assert_eq!(
            read["properties"]["encoding"]["enum"],
            serde_json::json!(["utf8", "base64"])
        );

        let log = parameters("git.log").unwrap();
        assert!(log["properties"]["ref"].is_object(), "serde rename honored");
        let stash = parameters("git.stash").unwrap();
        assert!(stash["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("action")));
        let stop = parameters("process.stop").unwrap();
        assert_eq!(stop["properties"]["id"]["type"], "string");
        let none = parameters("process.list").unwrap();
        assert_eq!(none["properties"], serde_json::json!({}));
        assert!(parameters("nope.nope").is_none());
    }
}
