"""
OpenScript config reader — reads API keys from mcp/assets/.openscript_config.json
first, then falls back to environment variables. This lets agents configure keys
at runtime via a config file without needing to set env vars on the MCP server process.
"""
import json
import os
from pathlib import Path

CONFIG_PATH = Path(__file__).parent.parent / "assets" / ".openscript_config.json"

def load_config():
    """Load the config file. Returns empty dict if not found."""
    try:
        with open(CONFIG_PATH) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}

def get_api_key(name):
    """Get an API key by name. Checks config file first, then env var.
    
    Args:
        name: The key name in both formats: 'pexels_api_key' in config,
              'PEXELS_API_KEY' in env vars.
    
    Returns:
        The API key string, or empty string if not found.
    """
    # Try config file first (snake_case)
    config = load_config()
    config_key = config.get(name, "")
    if config_key:
        return config_key
    
    # Fall back to env var (UPPER_SNAKE_CASE)
    env_name = name.upper()
    return os.environ.get(env_name, "")

def get_pexels_key():
    return get_api_key("pexels_api_key")

def get_giphy_key():
    return get_api_key("giphy_api_key")

def get_pixabay_key():
    return get_api_key("pixabay_api_key")
