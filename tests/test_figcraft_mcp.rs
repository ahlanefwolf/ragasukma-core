use ragasukma_core::agent::models::McpServer;

#[test]
fn test_compare_three_servers() {
    let playwright = McpServer {
        id: "playwright-1a1c2c19c35".to_string(),
        name: "playwright".to_string(),
        r#type: "stdio".to_string(),
        config: "using playwright mcp npx".to_string(),
        handshake: 1,
        created_at: 0,
        updated_at: 0,
    };

    let cartesia = McpServer {
        id: "cartesia-1a1c338f6fc".to_string(),
        name: "cartesia".to_string(),
        r#type: "http".to_string(),
        config: "url: https://mcp.cartesia.ai/mcp\napi key: sk_car_rtbhiq3ijVKbXHDUmd52YB".to_string(),
        handshake: 1,
        created_at: 0,
        updated_at: 0,
    };

    let figcraft = McpServer {
        id: "figcraft-1a1c3e7f7c2".to_string(),
        name: "figcraft".to_string(),
        r#type: "stdio".to_string(),
        config: "command: figcraft-cli mcp".to_string(),
        handshake: 1,
        created_at: 0,
        updated_at: 0,
    };

    println!("=== PLAYWRIGHT ===");
    println!("transport: {}", playwright.transport());
    println!("command: {:?}", playwright.command());
    println!("url: {:?}", playwright.url());

    println!("=== CARTESIA ===");
    println!("transport: {}", cartesia.transport());
    println!("command: {:?}", cartesia.command());
    println!("url: {:?}", cartesia.url());

    println!("=== FIGCRAFT ===");
    println!("transport: {}", figcraft.transport());
    println!("command: {:?}", figcraft.command());
    println!("url: {:?}", figcraft.url());
}
