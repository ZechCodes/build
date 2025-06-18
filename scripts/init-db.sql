-- Initialize Build Platform Database
-- This script runs when PostgreSQL container starts for the first time

-- Create extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pg_trgm";

-- Create indexes for text search
-- These will be created by alembic migrations, but we can prepare for them

-- Log the initialization
DO $$
BEGIN
    RAISE NOTICE 'Build Platform database initialized successfully';
END $$;