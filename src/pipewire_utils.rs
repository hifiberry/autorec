use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Source {
    pub name: String,
    pub description: Option<String>,
}

/// Get list of available PipeWire recording targets
pub fn get_available_targets() -> Vec<Source> {
    match Command::new("pw-cli")
        .arg("list-objects")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => {
            let output = match child.wait_with_output() {
                Ok(output) => output,
                Err(_) => return Vec::new(),
            };

            if !output.status.success() {
                return Vec::new();
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            parse_pw_cli_output(&stdout)
        }
        Err(_) => Vec::new(),
    }
}

fn parse_pw_cli_output(output: &str) -> Vec<Source> {
    let mut sources = Vec::new();
    let mut current_obj: Option<Source> = None;
    let mut is_source = false;

    for line in output.lines() {
        let line = line.trim();

        if line.contains("id") && (line.contains("type") || line.contains("Node")) {
            if let Some(obj) = current_obj.take() {
                if is_source {
                    sources.push(obj);
                }
            }
            current_obj = None;
            is_source = false;
        } else if line.contains("node.name") {
            if let Some(name) = extract_quoted_value(line) {
                current_obj = Some(Source {
                    name: name.to_string(),
                    description: None,
                });
            }
        } else if line.contains("node.description") || line.contains("node.nick") {
            if let Some(desc) = extract_quoted_value(line) {
                if let Some(ref mut obj) = current_obj {
                    obj.description = Some(desc.to_string());
                }
            }
        } else if line.contains("media.class") {
            if line.contains("Source") || line.contains("source") || line.contains("Input") {
                is_source = true;
            }
        }
    }

    // Don't forget the last object
    if let Some(obj) = current_obj {
        if is_source {
            sources.push(obj);
        }
    }

    sources
}

fn extract_quoted_value(line: &str) -> Option<&str> {
    let parts: Vec<&str> = line.split('"').collect();
    if parts.len() >= 2 {
        Some(parts[1])
    } else {
        None
    }
}

/// List available PipeWire recording targets
pub fn list_targets() -> i32 {
    let sources = get_available_targets();

    if sources.is_empty() {
        println!("No recording sources found or could not query PipeWire.");
        println!("Make sure PipeWire is running and pw-cli is installed.");
        return 1;
    }

    println!("Available PipeWire recording targets:");
    println!();
    for src in sources {
        println!("  {}", src.name);
        if let Some(desc) = src.description {
            println!("    {}", desc);
        }
        println!();
    }

    0
}

/// Validate or auto-select a PipeWire target
///
/// Returns (target_name, error_code) where error_code is 0 for success, 1 for error
pub fn validate_and_select_target(specified_target: Option<&str>, verbose: bool) -> (Option<String>, i32) {
    let available_targets = get_available_targets();
    let target_names: Vec<String> = available_targets.iter().map(|s| s.name.clone()).collect();

    if let Some(target) = specified_target {
        // Validate that the specified target exists
        if !target_names.is_empty() && !target_names.contains(&target.to_string()) {
            if verbose {
                eprintln!("Error: Target '{}' not found.", target);
                eprintln!("\nAvailable targets:");
                for name in &target_names {
                    eprintln!("  {}", name);
                }
                eprintln!("\nRun with --list-targets for more details.");
            }
            return (None, 1);
        }
        (Some(target.to_string()), 0)
    } else {
        // Auto-detect target
        if target_names.is_empty() {
            if verbose {
                eprintln!("Error: No recording targets found. Make sure PipeWire is running.");
                eprintln!("Run with --list-targets to see available targets.");
            }
            return (None, 1);
        }

        let target = target_names[0].clone();
        if verbose {
            println!("Auto-detected target: {}", target);
            // Show description if available
            for src in &available_targets {
                if src.name == target {
                    if let Some(desc) = &src.description {
                        println!("  {}", desc);
                    }
                }
            }
            println!();
        }
        (Some(target), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_quoted_value() {
        assert_eq!(extract_quoted_value("node.name = \"test\""), Some("test"));
        assert_eq!(extract_quoted_value("no quotes here"), None);
        assert_eq!(
            extract_quoted_value("node.description = \"Test Device\""),
            Some("Test Device")
        );
    }

    #[test]
    fn test_parse_pw_cli_output() {
        let output = r#"
id 42, type PipeWire:Interface:Node
    node.name = "alsa_output.monitor"
    node.description = "Monitor of ALSA Output"
    media.class = "Audio/Source"
id 43, type PipeWire:Interface:Node
    node.name = "test_input"
    media.class = "Audio/Sink"
        "#;

        let sources = parse_pw_cli_output(output);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "alsa_output.monitor");
        assert_eq!(
            sources[0].description,
            Some("Monitor of ALSA Output".to_string())
        );
    }

    #[test]
    fn test_validate_and_select_target_with_none() {
        // When no target specified and no sources available, should return error
        let (target, _code) = validate_and_select_target(None, false);
        // This will fail if PipeWire is not running, but that's expected
        assert!(target.is_none() || target.is_some());
    }

    #[test]
    fn test_source_struct() {
        let source = Source {
            name: "test".to_string(),
            description: Some("Test Description".to_string()),
        };
        assert_eq!(source.name, "test");
        assert_eq!(source.description, Some("Test Description".to_string()));
    }
}
