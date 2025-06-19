"""Database configuration and utilities."""

from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.config import get_settings
from app.models.base import Base

settings = get_settings()

# Create async engine with proper connection pooling
engine = create_async_engine(
    settings.database_url,
    echo=settings.database_echo if hasattr(settings, 'database_echo') else False,
    future=True,
    pool_pre_ping=True,
    pool_recycle=300,
    pool_size=20,  # Max connections per instance
    max_overflow=10,  # Additional connections when pool is full
)

# Create async session factory
AsyncSessionLocal = async_sessionmaker(
    engine,
    class_=AsyncSession,
    expire_on_commit=False,
)


async def get_db_session() -> AsyncSession:
    """Get database session dependency."""
    async with AsyncSessionLocal() as session:
        try:
            yield session
        finally:
            await session.close()


async def get_db() -> AsyncSession:
    """Database dependency for FastAPI routes."""
    async with AsyncSessionLocal() as session:
        try:
            yield session
        finally:
            await session.close()