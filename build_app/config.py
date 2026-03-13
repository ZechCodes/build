"""Configuration management using Pydantic Settings."""

from __future__ import annotations

from typing import Optional

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    """Application settings with environment-based configuration."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        case_sensitive=False,
        extra="ignore",
    )

    resend_api_key: Optional[str] = Field(
        default=None,
        description="Resend API key for transactional email",
    )
    site_base_url: str = Field(
        default="http://localhost:8000",
        description="Public base URL for the site (used in confirmation links)",
    )


_settings: Settings | None = None


def get_settings() -> Settings:
    """Get the global settings instance."""
    global _settings
    if _settings is None:
        _settings = Settings()
    return _settings
