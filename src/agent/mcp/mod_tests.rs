use super::*;

#[test]
fn test_parse_markdown_to_tool_def_basic() {
    let md = r#"# search_files
Search files across the repository.

## Parameters
- `path` (string, required): Directory path.
- `pattern` (string, optional): Search regex.

## Execution Contract
```json
{
  "method": "tools/call",
  "name": "search_files",
  "parameters": {
"type": "object",
"properties": {
  "path": { "type": "string" },
  "pattern": { "type": "string" }
},
"required": ["path"]
  }
}
```
"#;
    let def = parse_markdown_to_tool_def("search_files", md);
    assert_eq!(def.name, "search_files");
    assert!(def.description.unwrap().contains("Search files across"));
    assert_eq!(def.input_schema["type"], "object");
    assert!(def.input_schema["properties"]["path"].is_object());
    assert_eq!(def.input_schema["required"][0], "path");
}

#[test]
fn test_parse_markdown_to_tool_def_fallback() {
    let md = "# no_json_contract\nA simple tool without code block.";
    let def = parse_markdown_to_tool_def("no_json_contract", md);
    assert_eq!(def.name, "no_json_contract");
    assert!(def.description.unwrap().contains("A simple tool"));
    assert_eq!(def.input_schema["type"], "object");
}

#[test]
fn test_extract_command_template_basic() {
    let md = r#"# search_files
Search files.

## Parameters
- `path` (string, required): Directory path.

## Command Template
```bash
curl -s "https://api.example.com/search?q={path}"
```
"#;
    let template = extract_command_template(md).expect("template should be extracted");
    assert_eq!(
        template,
        r#"curl -s "https://api.example.com/search?q={path}""#
    );
}

#[test]
fn test_extract_command_template_returns_none_without_section() {
    let md = "# search_files\nNo command template here.";
    assert!(extract_command_template(md).is_none());
}

#[test]
fn test_extract_command_template_empty_block_returns_none() {
    let md = "## Command Template\n```bash\n```\n";
    assert!(extract_command_template(md).is_none());
}

#[test]
fn test_render_template_placeholders_string_number_bool() {
    let args = serde_json::json!({
        "query": "hello world",
        "limit": 10,
        "verbose": true,
        "extra": null
    });
    let rendered = render_template_placeholders(
        r#"curl -s "https://api.example.com/search?q={query}&limit={limit}&v={verbose}" {extra}"#,
        &args,
    );
    // `{extra}` (null) renders to an empty string, leaving the trailing space
    assert_eq!(
        rendered,
        r#"curl -s "https://api.example.com/search?q=hello world&limit=10&v=true" "#
    );
}

#[test]
fn test_render_template_placeholders_escapes_quotes_and_backslashes() {
    let args = serde_json::json!({ "path": "C:\\tmp\\a\"b" });
    let rendered = render_template_placeholders("python script --path {path}", &args);
    // backslash escaped then quote escaped
    assert_eq!(rendered, r#"python script --path C:\\tmp\\a\"b"#);
}

#[test]
fn test_render_template_placeholders_no_arguments_is_identity() {
    let template = "echo hello";
    assert_eq!(
        render_template_placeholders(template, &serde_json::Value::Null),
        template
    );
}

#[test]
fn test_normalize_command_for_windows() {
    // bash idioms that commonly break PowerShell 5.1
    let normalized = normalize_command_for_windows(r#"curl -s 'https://x' && python3 script.py"#);
    assert_eq!(normalized, r#"curl.exe -s "https://x" ; python script.py"#);
}

#[test]
fn test_normalize_command_for_windows_leaves_curl_exe_untouched() {
    let normalized = normalize_command_for_windows(r#"curl.exe -s "https://x""#);
    assert_eq!(normalized, r#"curl.exe -s "https://x""#);
}

#[test]
fn test_normalize_command_for_windows_replaces_only_bash_chain() {
    // ';' is already PowerShell-valid and must not be doubled
    let normalized = normalize_command_for_windows("cmd1 ; cmd2");
    assert_eq!(normalized, "cmd1 ; cmd2");
}
