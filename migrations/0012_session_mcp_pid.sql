-- Add pid column to session_mcp_servers to track OS process ID per session
ALTER TABLE session_mcp_servers ADD COLUMN pid INTEGER;
