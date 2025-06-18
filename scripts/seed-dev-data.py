#!/usr/bin/env python3
"""Seed development data for Build platform."""

import asyncio
import sys
from pathlib import Path

# Add the parent directory to sys.path to import app modules
sys.path.append(str(Path(__file__).parent.parent / "api"))

from sqlalchemy.ext.asyncio import AsyncSession
from passlib.context import CryptContext

from app.core.database import AsyncSessionLocal
from app.models import User, VM
from app.models.vm import VMStatus

# Password hashing
pwd_context = CryptContext(schemes=["bcrypt"], deprecated="auto")


async def create_dev_users(db: AsyncSession) -> None:
    """Create development users."""
    print("Creating development users...")
    
    # Check if admin user already exists
    existing_admin = await db.execute(
        "SELECT id FROM users WHERE email = 'admin@build-platform.dev'"
    )
    if existing_admin.fetchone():
        print("Admin user already exists, skipping user creation")
        return
    
    users_data = [
        {
            "email": "admin@build-platform.dev",
            "hashed_password": pwd_context.hash("admin123"),
            "full_name": "Admin User",
            "is_superuser": True,
        },
        {
            "email": "developer@build-platform.dev", 
            "hashed_password": pwd_context.hash("dev123"),
            "full_name": "Developer User",
            "is_superuser": False,
        },
        {
            "email": "test@build-platform.dev",
            "hashed_password": pwd_context.hash("test123"),
            "full_name": "Test User",
            "is_superuser": False,
        },
    ]
    
    for user_data in users_data:
        user = User(**user_data)
        db.add(user)
        print(f"Created user: {user_data['email']}")
    
    await db.commit()


async def create_dev_vms(db: AsyncSession) -> None:
    """Create development VMs."""
    print("Creating development VMs...")
    
    # Get the developer user
    result = await db.execute(
        "SELECT id FROM users WHERE email = 'developer@build-platform.dev'"
    )
    developer_user = result.fetchone()
    if not developer_user:
        print("Developer user not found, skipping VM creation")
        return
    
    developer_id = developer_user[0]
    
    vms_data = [
        {
            "name": "Ubuntu Development",
            "owner_id": developer_id,
            "status": VMStatus.STOPPED,
            "cpu_count": 2,
            "memory_mb": 1024,
            "disk_gb": 10,
            "description": "Ubuntu 22.04 development environment",
            "is_persistent": True,
        },
        {
            "name": "Node.js Environment",
            "owner_id": developer_id,
            "status": VMStatus.STOPPED,
            "cpu_count": 1,
            "memory_mb": 512,
            "disk_gb": 5,
            "description": "Node.js development environment with TypeScript",
            "is_persistent": True,
        },
    ]
    
    for vm_data in vms_data:
        vm = VM(**vm_data)
        db.add(vm)
        print(f"Created VM: {vm_data['name']}")
    
    await db.commit()


async def main():
    """Main seeding function."""
    print("🌱 Seeding development data for Build platform...")
    
    async with AsyncSessionLocal() as db:
        try:
            await create_dev_users(db)
            await create_dev_vms(db)
            print("✅ Development data seeded successfully!")
            
            print("\n📋 Development Credentials:")
            print("Admin: admin@build-platform.dev / admin123")
            print("Developer: developer@build-platform.dev / dev123")
            print("Test: test@build-platform.dev / test123")
            
        except Exception as e:
            print(f"❌ Error seeding data: {e}")
            await db.rollback()
            raise
        finally:
            await db.close()


if __name__ == "__main__":
    asyncio.run(main())