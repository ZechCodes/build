"""
Tests for automated backup encryption verification service.
"""

import pytest
import asyncio
import time
from unittest.mock import AsyncMock, MagicMock
from pathlib import Path
import sys

# Add parent directory to path for imports
current_dir = Path(__file__).parent.parent
if str(current_dir) not in sys.path:
    sys.path.insert(0, str(current_dir))

from security.backup_encryption_verifier import (
    BackupEncryptionVerifier,
    VerificationStatus,
    VerificationResult,
    VerificationReport
)


class TestBackupEncryptionVerifier:
    """Test cases for BackupEncryptionVerifier."""
    
    @pytest.fixture
    def mock_storage_backend(self):
        """Mock storage backend."""
        return AsyncMock()
    
    @pytest.fixture
    def mock_encryption_service(self):
        """Mock encryption service."""
        return AsyncMock()
    
    @pytest.fixture
    def verifier(self, mock_storage_backend, mock_encryption_service):
        """Create BackupEncryptionVerifier instance."""
        config = {
            "interval_hours": 1,
            "sample_percentage": 10.0,
            "max_concurrent": 2,
            "timeout_seconds": 60,
            "enable_alerting": True
        }
        return BackupEncryptionVerifier(
            storage_backend=mock_storage_backend,
            encryption_service=mock_encryption_service,
            verification_config=config
        )
    
    def test_verifier_initialization(self, verifier):
        """Test verifier initialization."""
        assert verifier.verification_interval_hours == 1
        assert verifier.sample_percentage == 10.0
        assert verifier.max_concurrent_verifications == 2
        assert verifier.verification_timeout_seconds == 60
        assert verifier.enable_alerting is True
        assert not verifier.is_running
    
    def test_get_verification_status(self, verifier):
        """Test verification status retrieval."""
        status = verifier.get_verification_status()
        
        assert isinstance(status, dict)
        assert "service_running" in status
        assert "verification_interval_hours" in status
        assert "sample_percentage" in status
        assert "active_verifications" in status
        assert "statistics" in status
        
        assert status["service_running"] is False
        assert status["verification_interval_hours"] == 1
        assert status["sample_percentage"] == 10.0
    
    @pytest.mark.asyncio
    async def test_verify_single_snapshot_success(self, verifier):
        """Test successful single snapshot verification."""
        snapshot_id = "test_snapshot_123"
        
        # Mock successful verification steps
        verifier._get_snapshot_metadata = AsyncMock(return_value={
            "snapshot_id": snapshot_id,
            "encryption_algorithm": "AES-256-GCM",
            "size_bytes": 1024
        })
        verifier._retrieve_snapshot_sample = AsyncMock(return_value=b"encrypted_data")
        verifier._verify_encryption_format = AsyncMock(return_value=True)
        verifier._verify_key_accessibility = AsyncMock(return_value=True)
        verifier._test_limited_decryption = AsyncMock(return_value=True)
        
        result = await verifier.verify_single_snapshot(snapshot_id)
        
        assert isinstance(result, VerificationResult)
        assert result.snapshot_id == snapshot_id
        assert result.status == VerificationStatus.SUCCESS
        assert result.key_accessible is True
        assert result.decryption_successful is True
        assert result.error_message is None
    
    @pytest.mark.asyncio
    async def test_verify_single_snapshot_failure(self, verifier):
        """Test failed single snapshot verification."""
        snapshot_id = "test_snapshot_fail"
        
        # Mock failed metadata retrieval
        verifier._get_snapshot_metadata = AsyncMock(return_value=None)
        
        result = await verifier.verify_single_snapshot(snapshot_id)
        
        assert isinstance(result, VerificationResult)
        assert result.snapshot_id == snapshot_id
        assert result.status == VerificationStatus.SKIPPED
        assert "metadata not accessible" in result.details
    
    @pytest.mark.asyncio
    async def test_verify_encrypted_backups_empty(self, verifier):
        """Test verification with no snapshots."""
        verifier._select_snapshots_for_verification = AsyncMock(return_value=[])
        
        report = await verifier.verify_encrypted_backups()
        
        assert isinstance(report, VerificationReport)
        assert report.total_snapshots_checked == 0
        assert report.successful_verifications == 0
        assert report.failed_verifications == 0
        assert report.verification_rate == 0.0
    
    @pytest.mark.asyncio
    async def test_verify_encrypted_backups_with_snapshots(self, verifier):
        """Test verification with multiple snapshots."""
        test_snapshots = ["snap1", "snap2", "snap3"]
        
        # Mock snapshot selection
        verifier._select_snapshots_for_verification = AsyncMock(return_value=test_snapshots)
        
        # Mock successful verification results
        success_result = VerificationResult(
            snapshot_id="test",
            verification_id="test_verify",
            status=VerificationStatus.SUCCESS,
            timestamp=time.time(),
            duration_seconds=1.0,
            details="Success"
        )
        
        verifier.verify_single_snapshot = AsyncMock(return_value=success_result)
        
        report = await verifier.verify_encrypted_backups()
        
        assert isinstance(report, VerificationReport)
        assert report.total_snapshots_checked == 3
        assert report.successful_verifications == 3
        assert report.failed_verifications == 0
        assert report.verification_rate == 1.0
    
    @pytest.mark.asyncio
    async def test_start_stop_background_verification(self, verifier):
        """Test starting and stopping background verification."""
        # Start background verification
        await verifier.start_background_verification()
        assert verifier.is_running is True
        assert verifier.background_task is not None
        
        # Stop background verification
        await verifier.stop_background_verification()
        assert verifier.is_running is False
    
    @pytest.mark.asyncio
    async def test_select_snapshots_for_verification(self, verifier):
        """Test snapshot selection logic."""
        # Mock 20 available snapshots
        mock_snapshots = [f"snapshot_{i}" for i in range(20)]
        verifier._get_all_encrypted_snapshots = AsyncMock(return_value=mock_snapshots)
        
        selected = await verifier._select_snapshots_for_verification()
        
        # Should select 10% of 20 = 2 snapshots (with min_sample_size = 1)
        assert len(selected) >= 1
        assert len(selected) <= verifier.max_sample_size
        assert all(snap in mock_snapshots for snap in selected)
    
    def test_encryption_format_validation(self, verifier):
        """Test encryption format validation."""
        # Test valid encrypted data
        valid_data = b"encrypted_random_data_with_sufficient_entropy"
        result = asyncio.run(verifier._verify_encryption_format(valid_data))
        assert result is True
        
        # Test invalid data (too short)
        invalid_data = b"short"
        result = asyncio.run(verifier._verify_encryption_format(invalid_data))
        assert result is False
        
        # Test suspicious unencrypted patterns
        suspicious_data = b"<html><body>clearly not encrypted</body></html>"
        result = asyncio.run(verifier._verify_encryption_format(suspicious_data))
        assert result is False
    
    @pytest.mark.asyncio
    async def test_key_accessibility_verification(self, verifier):
        """Test key accessibility verification."""
        # Test successful key access
        result = await verifier._verify_key_accessibility("test_snapshot")
        assert result is True  # Mock implementation returns True
    
    @pytest.mark.asyncio
    async def test_limited_decryption_test(self, verifier):
        """Test limited decryption testing."""
        test_data = b"encrypted_test_data"
        result = await verifier._test_limited_decryption("test_snapshot", test_data)
        assert result is True  # Mock implementation returns True for non-empty data
        
        # Test with empty data
        result = await verifier._test_limited_decryption("test_snapshot", b"")
        assert result is False
    
    def test_verification_state_tracking(self, verifier):
        """Test verification state tracking."""
        # Test initial state
        assert len(verifier.verification_history) == 0
        assert len(verifier.active_verifications) == 0
        assert verifier.last_verification_time == 0
        
        # Test statistics initialization
        stats = verifier.verification_stats
        assert stats["total_verifications"] == 0
        assert stats["successful_verifications"] == 0
        assert stats["failed_verifications"] == 0
        assert stats["average_duration"] == 0.0
    
    @pytest.mark.asyncio
    async def test_get_all_encrypted_snapshots(self, verifier):
        """Test getting all encrypted snapshots."""
        # Test mock implementation
        snapshots = await verifier._get_all_encrypted_snapshots()
        assert isinstance(snapshots, list)
        assert len(snapshots) == 10  # Mock returns 10 snapshots
        assert all(snap.startswith("snap_vm_001_user_123_") for snap in snapshots)
    
    @pytest.mark.asyncio
    async def test_get_snapshot_metadata(self, verifier):
        """Test snapshot metadata retrieval."""
        metadata = await verifier._get_snapshot_metadata("test_snapshot")
        assert isinstance(metadata, dict)
        assert metadata["snapshot_id"] == "test_snapshot"
        assert metadata["encryption_algorithm"] == "AES-256-GCM"
        assert "created_at" in metadata
        assert "size_bytes" in metadata
        assert "checksum" in metadata
    
    @pytest.mark.asyncio
    async def test_retrieve_snapshot_sample(self, verifier):
        """Test snapshot sample retrieval."""
        sample = await verifier._retrieve_snapshot_sample("test_snapshot")
        assert isinstance(sample, bytes)
        assert b"encrypted_sample_data_test_snapshot" == sample


if __name__ == "__main__":
    pytest.main([__file__, "-v"])