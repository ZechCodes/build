#!/usr/bin/env python3
"""Create a demo user in the database for development."""

import asyncio
import uuid
from sqlalchemy.ext.asyncio import AsyncSession
from app.core.database import AsyncSessionLocal
from app.models.user import User, UserRole
from app.core.security import get_password_hash


async def create_demo_user():
    """Create a demo user with the same ID used in JWT tokens."""
    
    demo_user_id = uuid.UUID("550e8400-e29b-41d4-a716-446655440000")
    
    async with AsyncSessionLocal() as db:
        # Check if demo user already exists
        existing_user = await db.get(User, demo_user_id)
        if existing_user:
            print(f"Demo user already exists: {existing_user.email}")
            return existing_user
        
        # Create demo user
        demo_user = User(
            id=demo_user_id,
            email="demo@example.com",
            username="demo",
            password_hash=get_password_hash("demo123"),  # Use proper password hash
            role=UserRole.USER,
            is_active=True,
            is_verified=True
        )
        
        db.add(demo_user)
        await db.commit()
        await db.refresh(demo_user)
        
        print(f"Demo user created successfully:")
        print(f"  ID: {demo_user.id}")
        print(f"  Email: {demo_user.email}")
        print(f"  Username: {demo_user.username}")
        print(f"  Role: {demo_user.role.value}")
        
        return demo_user


if __name__ == "__main__":
    asyncio.run(create_demo_user())