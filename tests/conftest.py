"""Integration test configuration."""

import asyncio
import pytest
import httpx
from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker

from api.app.core.database import Base


# Test database for integration tests
TEST_DATABASE_URL = "postgresql://postgres:dev_password@localhost:5434/build_test"

# Test engine
test_engine = create_async_engine(TEST_DATABASE_URL, echo=False)
TestSessionLocal = async_sessionmaker(
    test_engine,
    class_=AsyncSession,
    expire_on_commit=False,
)


@pytest.fixture(scope="session")
def event_loop():
    """Create an instance of the default event loop for the test session."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(scope="session")
async def setup_test_database():
    """Set up test database."""
    async with test_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    yield
    async with test_engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all)


@pytest.fixture
async def db_session(setup_test_database):
    """Create a test database session."""
    async with TestSessionLocal() as session:
        yield session
        await session.rollback()


@pytest.fixture(scope="session")
async def api_client():
    """Create HTTP client for API testing."""
    async with httpx.AsyncClient(base_url="http://localhost:8000") as client:
        yield client


@pytest.fixture
def test_user_data():
    """Test user data."""
    return {
        "email": "integration_test@example.com",
        "password": "testpassword123",
        "full_name": "Integration Test User",
    }


@pytest.fixture
async def authenticated_user(api_client, test_user_data):
    """Create authenticated user and return tokens."""
    # Register user
    register_response = await api_client.post(
        "/api/v1/auth/register", 
        json=test_user_data
    )
    assert register_response.status_code == 200
    
    # Login
    login_data = {
        "email": test_user_data["email"],
        "password": test_user_data["password"]
    }
    login_response = await api_client.post(
        "/api/v1/auth/login", 
        json=login_data
    )
    assert login_response.status_code == 200
    
    tokens = login_response.json()
    return {
        "user": register_response.json(),
        "access_token": tokens["access_token"],
        "refresh_token": tokens["refresh_token"],
    }