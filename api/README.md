# API Service

## Overview
Core FastAPI backend service that provides the main REST API for the Build platform. Handles user management, VM lifecycle, session management, and serves as the primary interface for frontend applications.

## Structure
```
api/
├── main.py                 # FastAPI application entry point
├── database/               # Database configuration and migrations
├── models/                 # SQLAlchemy models
├── schemas/                # Pydantic schemas for API
├── routers/                # API route handlers
├── dependencies/           # FastAPI dependencies
├── middleware/             # Custom middleware
├── services/               # Business logic services
├── utils/                  # Utility functions
├── tests/                  # Unit and integration tests
└── requirements.txt        # Python dependencies
```

## Key Responsibilities
- User authentication and authorization
- VM instance management
- Session lifecycle management
- Snapshot operations
- Git repository integration
- Recording management
- Rate limiting and security

## Dependencies
- FastAPI: Web framework
- SQLAlchemy: ORM and database toolkit
- Pydantic: Data validation and serialization
- Redis: Caching and session storage
- PostgreSQL: Primary database

## Configuration
Environment variables and configuration managed through Pydantic settings model.

## Development
See Session 1 documentation for detailed implementation guidance.