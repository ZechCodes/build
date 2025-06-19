"""Test configuration and fixtures."""

import asyncio
import pytest
import pytest_asyncio
from httpx import AsyncClient
from sqlalchemy.ext.asyncio import AsyncSession, create_async_engine, async_sessionmaker
from sqlalchemy.pool import StaticPool

from app.models.base import Base


# Test database URL
TEST_DATABASE_URL = "sqlite+aiosqlite:///./test.db"

# Create test engine
test_engine = create_async_engine(
    TEST_DATABASE_URL,
    poolclass=StaticPool,
    connect_args={"check_same_thread": False},
    echo=False,
)

# Create test session factory
TestSessionLocal = async_sessionmaker(
    test_engine,
    class_=AsyncSession,
    expire_on_commit=False,
)


@pytest_asyncio.fixture(scope="session")
def event_loop():
    """Create an instance of the default event loop for the test session."""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest_asyncio.fixture
async def db_session():
    """Create a test database session."""
    async with test_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    
    async with TestSessionLocal() as session:
        yield session
    
    async with test_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all)


@pytest_asyncio.fixture
async def client(db_session):
    """Create a test client."""
    # Create a simplified app for testing without complex middleware
    from fastapi import FastAPI
    from app.api.v1.api import api_router
    from app.core.deps import get_db
    from httpx import ASGITransport
    
    # Create simple test app
    test_app = FastAPI(title="Test API")
    test_app.include_router(api_router, prefix="/api/v1")
    
    async def override_get_db():
        yield db_session

    test_app.dependency_overrides[get_db] = override_get_db
    
    transport = ASGITransport(app=test_app)
    async with AsyncClient(transport=transport, base_url="http://test") as ac:
        yield ac
    
    test_app.dependency_overrides.clear()


@pytest.fixture
def test_user_data():
    """Test user data fixture."""
    return {
        "email": "test@example.com",
        "password": "TestPassword123!",
        "username": "testuser",
    }


@pytest.fixture
def test_admin_data():
    """Test admin user data fixture."""
    return {
        "email": "admin@example.com",
        "password": "AdminPassword123!",
        "username": "admin",
    }