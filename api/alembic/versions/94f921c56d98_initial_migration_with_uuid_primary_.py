"""Initial migration with UUID primary keys and new models

Revision ID: 94f921c56d98
Revises: 
Create Date: 2025-06-18 18:39:56.570333

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql


# revision identifiers, used by Alembic.
revision: str = '94f921c56d98'
down_revision: Union[str, Sequence[str], None] = None
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    """Upgrade schema."""
    # Enable UUID extension
    op.execute('CREATE EXTENSION IF NOT EXISTS "uuid-ossp"')
    
    # Enable Row-Level Security
    op.execute('CREATE EXTENSION IF NOT EXISTS "pgcrypto"')
    
    # Create users table
    op.create_table(
        'users',
        sa.Column('id', postgresql.UUID(as_uuid=True), primary_key=True, 
                 server_default=sa.text('uuid_generate_v4()'), nullable=False),
        sa.Column('email', sa.String(255), unique=True, nullable=False),
        sa.Column('username', sa.String(50), unique=True, nullable=False),
        sa.Column('password_hash', sa.String(255), nullable=False),
        sa.Column('is_active', sa.Boolean(), default=True, nullable=False),
        sa.Column('is_verified', sa.Boolean(), default=False, nullable=False),
        sa.Column('created_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
    )
    
    # Create indexes for users table
    op.create_index('ix_users_email', 'users', ['email'])
    op.create_index('ix_users_username', 'users', ['username'])
    op.create_index('ix_users_is_active', 'users', ['is_active'])
    op.create_index('ix_users_created_at', 'users', ['created_at'])
    
    # Create vm_instances table
    op.create_table(
        'vm_instances',
        sa.Column('id', postgresql.UUID(as_uuid=True), primary_key=True, 
                 server_default=sa.text('uuid_generate_v4()'), nullable=False),
        sa.Column('user_id', postgresql.UUID(as_uuid=True), 
                 sa.ForeignKey('users.id', ondelete='CASCADE'), nullable=False),
        sa.Column('name', sa.String(100), nullable=False),
        sa.Column('state', sa.String(20), nullable=False, default='stopped'),
        sa.Column('firecracker_id', sa.String(50), unique=True, nullable=True),
        sa.Column('config', postgresql.JSON, nullable=False),
        sa.Column('created_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
    )
    
    # Create indexes for vm_instances table
    op.create_index('ix_vm_instances_user_id', 'vm_instances', ['user_id'])
    op.create_index('ix_vm_instances_state', 'vm_instances', ['state'])
    op.create_index('ix_vm_instances_firecracker_id', 'vm_instances', ['firecracker_id'])
    op.create_index('ix_vm_instances_created_at', 'vm_instances', ['created_at'])
    
    # Create sessions table
    op.create_table(
        'sessions',
        sa.Column('id', postgresql.UUID(as_uuid=True), primary_key=True, 
                 server_default=sa.text('uuid_generate_v4()'), nullable=False),
        sa.Column('vm_instance_id', postgresql.UUID(as_uuid=True), 
                 sa.ForeignKey('vm_instances.id', ondelete='CASCADE'), nullable=False),
        sa.Column('user_id', postgresql.UUID(as_uuid=True), 
                 sa.ForeignKey('users.id', ondelete='CASCADE'), nullable=False),
        sa.Column('session_token', sa.String(255), unique=True, nullable=False),
        sa.Column('state', sa.String(20), nullable=False, default='active'),
        sa.Column('last_activity', sa.DateTime(timezone=True), nullable=False),
        sa.Column('expires_at', sa.DateTime(timezone=True), nullable=False),
        sa.Column('created_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
    )
    
    # Create indexes for sessions table
    op.create_index('ix_sessions_vm_instance_id', 'sessions', ['vm_instance_id'])
    op.create_index('ix_sessions_user_id', 'sessions', ['user_id'])
    op.create_index('ix_sessions_session_token', 'sessions', ['session_token'])
    op.create_index('ix_sessions_state', 'sessions', ['state'])
    op.create_index('ix_sessions_expires_at', 'sessions', ['expires_at'])
    
    # Create snapshots table
    op.create_table(
        'snapshots',
        sa.Column('id', postgresql.UUID(as_uuid=True), primary_key=True, 
                 server_default=sa.text('uuid_generate_v4()'), nullable=False),
        sa.Column('vm_instance_id', postgresql.UUID(as_uuid=True), 
                 sa.ForeignKey('vm_instances.id', ondelete='CASCADE'), nullable=False),
        sa.Column('user_id', postgresql.UUID(as_uuid=True), 
                 sa.ForeignKey('users.id', ondelete='CASCADE'), nullable=False),
        sa.Column('name', sa.String(100), nullable=False),
        sa.Column('description', sa.Text, nullable=True),
        sa.Column('storage_path', sa.String(500), nullable=False),
        sa.Column('size_bytes', sa.BigInteger, nullable=False),
        sa.Column('snapshot_metadata', postgresql.JSON, nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
    )
    
    # Create indexes for snapshots table
    op.create_index('ix_snapshots_vm_instance_id', 'snapshots', ['vm_instance_id'])
    op.create_index('ix_snapshots_user_id', 'snapshots', ['user_id'])
    op.create_index('ix_snapshots_name', 'snapshots', ['name'])
    op.create_index('ix_snapshots_created_at', 'snapshots', ['created_at'])
    
    # Create audit_logs table
    op.create_table(
        'audit_logs',
        sa.Column('id', postgresql.UUID(as_uuid=True), primary_key=True, 
                 server_default=sa.text('uuid_generate_v4()'), nullable=False),
        sa.Column('user_id', postgresql.UUID(as_uuid=True), 
                 sa.ForeignKey('users.id'), nullable=True),
        sa.Column('action', sa.String(100), nullable=False),
        sa.Column('resource_type', sa.String(50), nullable=True),
        sa.Column('resource_id', postgresql.UUID(as_uuid=True), nullable=True),
        sa.Column('ip_address', sa.String(45), nullable=True),
        sa.Column('user_agent', sa.Text, nullable=True),
        sa.Column('details', postgresql.JSON, nullable=True),
        sa.Column('created_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
        sa.Column('updated_at', sa.DateTime(timezone=True), 
                 server_default=sa.func.now(), nullable=False),
    )
    
    # Create indexes for audit_logs table
    op.create_index('ix_audit_logs_user_id', 'audit_logs', ['user_id'])
    op.create_index('ix_audit_logs_action', 'audit_logs', ['action'])
    op.create_index('ix_audit_logs_resource_type', 'audit_logs', ['resource_type'])
    op.create_index('ix_audit_logs_resource_id', 'audit_logs', ['resource_id'])
    op.create_index('ix_audit_logs_created_at', 'audit_logs', ['created_at'])
    
    # Enable Row-Level Security (RLS) on all tables
    op.execute('ALTER TABLE users ENABLE ROW LEVEL SECURITY')
    op.execute('ALTER TABLE vm_instances ENABLE ROW LEVEL SECURITY')
    op.execute('ALTER TABLE sessions ENABLE ROW LEVEL SECURITY')
    op.execute('ALTER TABLE snapshots ENABLE ROW LEVEL SECURITY')
    op.execute('ALTER TABLE audit_logs ENABLE ROW LEVEL SECURITY')
    
    # Create RLS policies for users table
    op.execute('''
        CREATE POLICY users_select_own ON users
        FOR SELECT USING (id = current_setting('app.current_user_id')::uuid)
    ''')
    
    op.execute('''
        CREATE POLICY users_update_own ON users
        FOR UPDATE USING (id = current_setting('app.current_user_id')::uuid)
    ''')
    
    # Create RLS policies for vm_instances table
    op.execute('''
        CREATE POLICY vm_instances_all_own ON vm_instances
        FOR ALL USING (user_id = current_setting('app.current_user_id')::uuid)
    ''')
    
    # Create RLS policies for sessions table
    op.execute('''
        CREATE POLICY sessions_all_own ON sessions
        FOR ALL USING (user_id = current_setting('app.current_user_id')::uuid)
    ''')
    
    # Create RLS policies for snapshots table
    op.execute('''
        CREATE POLICY snapshots_all_own ON snapshots
        FOR ALL USING (user_id = current_setting('app.current_user_id')::uuid)
    ''')
    
    # Create RLS policies for audit_logs table (read-only for own records)
    op.execute('''
        CREATE POLICY audit_logs_select_own ON audit_logs
        FOR SELECT USING (user_id = current_setting('app.current_user_id')::uuid OR user_id IS NULL)
    ''')
    
    # Create updated_at trigger function
    op.execute('''
        CREATE OR REPLACE FUNCTION update_updated_at_column()
        RETURNS TRIGGER AS $$
        BEGIN
            NEW.updated_at = CURRENT_TIMESTAMP;
            RETURN NEW;
        END;
        $$ language 'plpgsql';
    ''')
    
    # Create triggers for updated_at columns
    op.execute('''
        CREATE TRIGGER update_users_updated_at BEFORE UPDATE ON users
        FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
    ''')
    
    op.execute('''
        CREATE TRIGGER update_vm_instances_updated_at BEFORE UPDATE ON vm_instances
        FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
    ''')
    
    op.execute('''
        CREATE TRIGGER update_sessions_updated_at BEFORE UPDATE ON sessions
        FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
    ''')
    
    op.execute('''
        CREATE TRIGGER update_snapshots_updated_at BEFORE UPDATE ON snapshots
        FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
    ''')
    
    op.execute('''
        CREATE TRIGGER update_audit_logs_updated_at BEFORE UPDATE ON audit_logs
        FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
    ''')


def downgrade() -> None:
    """Downgrade schema."""
    # Drop triggers
    op.execute('DROP TRIGGER IF EXISTS update_audit_logs_updated_at ON audit_logs')
    op.execute('DROP TRIGGER IF EXISTS update_snapshots_updated_at ON snapshots')
    op.execute('DROP TRIGGER IF EXISTS update_sessions_updated_at ON sessions')
    op.execute('DROP TRIGGER IF EXISTS update_vm_instances_updated_at ON vm_instances')
    op.execute('DROP TRIGGER IF EXISTS update_users_updated_at ON users')
    
    # Drop trigger function
    op.execute('DROP FUNCTION IF EXISTS update_updated_at_column()')
    
    # Drop tables (cascade will handle foreign keys)
    op.drop_table('audit_logs')
    op.drop_table('snapshots')
    op.drop_table('sessions')
    op.drop_table('vm_instances')
    op.drop_table('users')
