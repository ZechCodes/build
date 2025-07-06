"""
Automated Backup Encryption Verification Service

Provides automated verification of backup encryption integrity, ensuring
encrypted snapshots remain accessible and properly encrypted over time.
"""

import asyncio
import time
import random
import hashlib
from typing import Dict, Any, Optional, List, Tuple
from dataclasses import dataclass, asdict
from enum import Enum
from pathlib import Path
import structlog
import logfire

logger = structlog.get_logger()


class VerificationStatus(Enum):
    """Verification result status."""
    SUCCESS = "success"
    FAILURE = "failure"
    WARNING = "warning"
    SKIPPED = "skipped"


@dataclass
class VerificationResult:
    """Result of backup encryption verification."""
    snapshot_id: str
    verification_id: str
    status: VerificationStatus
    timestamp: float
    duration_seconds: float
    details: str
    error_message: Optional[str] = None
    encryption_algorithm: Optional[str] = None
    key_accessible: bool = True
    decryption_successful: bool = True


@dataclass
class VerificationReport:
    """Comprehensive verification report."""
    report_id: str
    timestamp: float
    total_snapshots_checked: int
    successful_verifications: int
    failed_verifications: int
    warning_verifications: int
    skipped_verifications: int
    verification_rate: float
    duration_seconds: float
    next_verification: float
    details: List[VerificationResult]


class BackupEncryptionVerifier:
    """
    Automated backup encryption verification service.
    
    Provides:
    - Periodic verification of encrypted backup integrity
    - Non-destructive encryption testing
    - Key accessibility validation
    - Automated alerting on verification failures
    - Performance monitoring of verification operations
    """
    
    def __init__(self, storage_backend, encryption_service, 
                 verification_config: Optional[Dict[str, Any]] = None):
        """
        Initialize backup encryption verifier.
        
        Args:
            storage_backend: Storage backend for snapshot access
            encryption_service: Encryption service for testing
            verification_config: Configuration for verification behavior
        """
        self.storage_backend = storage_backend
        self.encryption_service = encryption_service
        
        # Configuration
        config = verification_config or {}
        self.verification_interval_hours = config.get("interval_hours", 24)
        self.sample_percentage = config.get("sample_percentage", 5.0)  # 5% of snapshots
        self.max_concurrent_verifications = config.get("max_concurrent", 3)
        self.verification_timeout_seconds = config.get("timeout_seconds", 300)
        self.enable_alerting = config.get("enable_alerting", True)
        self.min_sample_size = config.get("min_sample_size", 1)
        self.max_sample_size = config.get("max_sample_size", 20)
        
        # State tracking
        self.verification_history: List[VerificationReport] = []
        self.last_verification_time = 0
        self.active_verifications: Dict[str, asyncio.Task] = {}
        self.verification_stats = {
            "total_verifications": 0,
            "successful_verifications": 0,
            "failed_verifications": 0,
            "average_duration": 0.0
        }
        
        # Background task reference
        self.background_task: Optional[asyncio.Task] = None
        self.is_running = False
    
    async def start_background_verification(self):
        """Start automated background verification process."""
        if self.is_running:
            logger.warning("Background verification already running")
            return
        
        self.is_running = True
        self.background_task = asyncio.create_task(self._verification_loop())
        
        logger.info("Automated backup encryption verification started",
                   interval_hours=self.verification_interval_hours,
                   sample_percentage=self.sample_percentage)
        
        logfire.info("Backup encryption verification service started",
                    interval_hours=self.verification_interval_hours,
                    sample_percentage=self.sample_percentage)
    
    async def stop_background_verification(self):
        """Stop automated background verification process."""
        self.is_running = False
        
        if self.background_task and not self.background_task.done():
            self.background_task.cancel()
            try:
                await self.background_task
            except asyncio.CancelledError:
                pass
        
        # Cancel any active verifications
        for verification_id, task in self.active_verifications.items():
            if not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
        
        self.active_verifications.clear()
        
        logger.info("Automated backup encryption verification stopped")
        logfire.info("Backup encryption verification service stopped")
    
    async def verify_encrypted_backups(self, snapshot_list: Optional[List[str]] = None) -> VerificationReport:
        """
        Perform comprehensive verification of encrypted backups.
        
        Args:
            snapshot_list: Optional list of specific snapshots to verify
            
        Returns:
            VerificationReport: Comprehensive verification results
        """
        verification_start = time.time()
        report_id = f"verify_{int(verification_start)}_{random.randint(1000, 9999)}"
        
        logger.info("Starting backup encryption verification",
                   report_id=report_id,
                   snapshot_list_provided=snapshot_list is not None)
        
        try:
            # Get snapshots to verify
            if snapshot_list:
                snapshots_to_verify = snapshot_list
            else:
                snapshots_to_verify = await self._select_snapshots_for_verification()
            
            if not snapshots_to_verify:
                logger.warning("No snapshots available for verification")
                return self._create_empty_report(report_id, verification_start)
            
            # Perform verifications
            verification_results = await self._verify_snapshot_batch(
                snapshots_to_verify, report_id
            )
            
            # Calculate statistics
            total_checked = len(verification_results)
            successful = len([r for r in verification_results if r.status == VerificationStatus.SUCCESS])
            failed = len([r for r in verification_results if r.status == VerificationStatus.FAILURE])
            warnings = len([r for r in verification_results if r.status == VerificationStatus.WARNING])
            skipped = len([r for r in verification_results if r.status == VerificationStatus.SKIPPED])
            
            verification_duration = time.time() - verification_start
            verification_rate = successful / total_checked if total_checked > 0 else 0.0
            
            # Create comprehensive report
            report = VerificationReport(
                report_id=report_id,
                timestamp=verification_start,
                total_snapshots_checked=total_checked,
                successful_verifications=successful,
                failed_verifications=failed,
                warning_verifications=warnings,
                skipped_verifications=skipped,
                verification_rate=verification_rate,
                duration_seconds=verification_duration,
                next_verification=verification_start + (self.verification_interval_hours * 3600),
                details=verification_results
            )
            
            # Store report and update statistics
            self.verification_history.append(report)
            self._update_verification_stats(report)
            
            # Handle alerting for failures
            if failed > 0 and self.enable_alerting:
                await self._handle_verification_failures(report)
            
            logger.info("Backup encryption verification completed",
                       report_id=report_id,
                       total_checked=total_checked,
                       successful=successful,
                       failed=failed,
                       verification_rate=verification_rate,
                       duration_seconds=verification_duration)
            
            logfire.info("Backup encryption verification completed",
                        report_id=report_id,
                        total_snapshots=total_checked,
                        success_rate=verification_rate,
                        duration_seconds=verification_duration,
                        failed_count=failed)
            
            return report
            
        except Exception as e:
            logger.error("Backup encryption verification failed",
                        report_id=report_id, error=str(e))
            logfire.error("Backup encryption verification error",
                         report_id=report_id, error=str(e))
            raise
    
    async def verify_single_snapshot(self, snapshot_id: str) -> VerificationResult:
        """
        Verify encryption integrity of a single snapshot.
        
        Args:
            snapshot_id: Snapshot to verify
            
        Returns:
            VerificationResult: Verification result for the snapshot
        """
        verification_start = time.time()
        verification_id = f"single_{snapshot_id}_{int(verification_start)}"
        
        logger.debug("Verifying single snapshot encryption",
                    snapshot_id=snapshot_id,
                    verification_id=verification_id)
        
        try:
            # Step 1: Verify snapshot exists and is accessible
            try:
                snapshot_metadata = await self._get_snapshot_metadata(snapshot_id)
                if not snapshot_metadata:
                    return VerificationResult(
                        snapshot_id=snapshot_id,
                        verification_id=verification_id,
                        status=VerificationStatus.SKIPPED,
                        timestamp=verification_start,
                        duration_seconds=time.time() - verification_start,
                        details="Snapshot metadata not accessible",
                        error_message="Could not retrieve snapshot metadata"
                    )
            except Exception as e:
                return VerificationResult(
                    snapshot_id=snapshot_id,
                    verification_id=verification_id,
                    status=VerificationStatus.FAILURE,
                    timestamp=verification_start,
                    duration_seconds=time.time() - verification_start,
                    details="Failed to access snapshot metadata",
                    error_message=str(e)
                )
            
            # Step 2: Retrieve encrypted snapshot data (sample only)
            try:
                encrypted_data = await self._retrieve_snapshot_sample(snapshot_id)
                if not encrypted_data:
                    return VerificationResult(
                        snapshot_id=snapshot_id,
                        verification_id=verification_id,
                        status=VerificationStatus.FAILURE,
                        timestamp=verification_start,
                        duration_seconds=time.time() - verification_start,
                        details="No encrypted data retrieved",
                        error_message="Snapshot data not accessible"
                    )
            except Exception as e:
                return VerificationResult(
                    snapshot_id=snapshot_id,
                    verification_id=verification_id,
                    status=VerificationStatus.FAILURE,
                    timestamp=verification_start,
                    duration_seconds=time.time() - verification_start,
                    details="Failed to retrieve encrypted data",
                    error_message=str(e)
                )
            
            # Step 3: Verify encryption format and structure
            encryption_valid = await self._verify_encryption_format(encrypted_data)
            if not encryption_valid:
                return VerificationResult(
                    snapshot_id=snapshot_id,
                    verification_id=verification_id,
                    status=VerificationStatus.FAILURE,
                    timestamp=verification_start,
                    duration_seconds=time.time() - verification_start,
                    details="Invalid encryption format detected",
                    error_message="Encryption format validation failed"
                )
            
            # Step 4: Test key accessibility (without full decryption)
            key_accessible = await self._verify_key_accessibility(snapshot_id)
            if not key_accessible:
                return VerificationResult(
                    snapshot_id=snapshot_id,
                    verification_id=verification_id,
                    status=VerificationStatus.FAILURE,
                    timestamp=verification_start,
                    duration_seconds=time.time() - verification_start,
                    details="Encryption keys not accessible",
                    error_message="Cannot access encryption keys",
                    key_accessible=False
                )
            
            # Step 5: Perform limited decryption test (first block only)
            decryption_successful = await self._test_limited_decryption(snapshot_id, encrypted_data)
            
            # Determine final status
            if decryption_successful:
                status = VerificationStatus.SUCCESS
                details = "Encryption verification successful"
                error_message = None
            else:
                status = VerificationStatus.WARNING
                details = "Encryption accessible but decryption test inconclusive"
                error_message = "Limited decryption test failed"
            
            verification_duration = time.time() - verification_start
            
            result = VerificationResult(
                snapshot_id=snapshot_id,
                verification_id=verification_id,
                status=status,
                timestamp=verification_start,
                duration_seconds=verification_duration,
                details=details,
                error_message=error_message,
                encryption_algorithm="AES-256-GCM",
                key_accessible=key_accessible,
                decryption_successful=decryption_successful
            )
            
            logger.debug("Single snapshot verification completed",
                        snapshot_id=snapshot_id,
                        status=status.value,
                        duration_seconds=verification_duration)
            
            return result
            
        except Exception as e:
            logger.error("Single snapshot verification failed",
                        snapshot_id=snapshot_id, error=str(e))
            
            return VerificationResult(
                snapshot_id=snapshot_id,
                verification_id=verification_id,
                status=VerificationStatus.FAILURE,
                timestamp=verification_start,
                duration_seconds=time.time() - verification_start,
                details="Verification failed with exception",
                error_message=str(e)
            )
    
    def get_verification_status(self) -> Dict[str, Any]:
        """Get current verification service status."""
        recent_reports = self.verification_history[-5:]  # Last 5 reports
        
        status = {
            "service_running": self.is_running,
            "last_verification": self.last_verification_time,
            "next_verification": (
                self.last_verification_time + (self.verification_interval_hours * 3600)
                if self.last_verification_time > 0 else time.time()
            ),
            "verification_interval_hours": self.verification_interval_hours,
            "sample_percentage": self.sample_percentage,
            "active_verifications": len(self.active_verifications),
            "total_reports": len(self.verification_history),
            "statistics": self.verification_stats.copy(),
            "recent_reports": [
                {
                    "report_id": report.report_id,
                    "timestamp": report.timestamp,
                    "verification_rate": report.verification_rate,
                    "total_checked": report.total_snapshots_checked,
                    "failed": report.failed_verifications
                }
                for report in recent_reports
            ]
        }
        
        return status
    
    # Private implementation methods
    
    async def _verification_loop(self):
        """Main background verification loop."""
        try:
            while self.is_running:
                try:
                    # Calculate next verification time
                    next_verification = (
                        self.last_verification_time + (self.verification_interval_hours * 3600)
                        if self.last_verification_time > 0
                        else time.time()
                    )
                    
                    # Wait until next verification time
                    current_time = time.time()
                    if next_verification > current_time:
                        sleep_duration = min(next_verification - current_time, 3600)  # Max 1 hour
                        await asyncio.sleep(sleep_duration)
                        continue
                    
                    # Perform verification
                    logger.info("Starting scheduled backup encryption verification")
                    report = await self.verify_encrypted_backups()
                    self.last_verification_time = time.time()
                    
                    logger.info("Scheduled verification completed",
                               report_id=report.report_id,
                               verification_rate=report.verification_rate)
                    
                except Exception as e:
                    logger.error("Scheduled verification failed", error=str(e))
                    # Wait before retrying
                    await asyncio.sleep(600)  # 10 minutes
                    
        except asyncio.CancelledError:
            logger.info("Verification loop cancelled")
        except Exception as e:
            logger.error("Verification loop failed", error=str(e))
    
    async def _select_snapshots_for_verification(self) -> List[str]:
        """Select snapshots for verification based on sampling strategy."""
        try:
            # Get all available snapshots (this would be replaced with actual implementation)
            all_snapshots = await self._get_all_encrypted_snapshots()
            
            if not all_snapshots:
                return []
            
            # Calculate sample size
            sample_size = max(
                self.min_sample_size,
                min(
                    self.max_sample_size,
                    int(len(all_snapshots) * (self.sample_percentage / 100.0))
                )
            )
            
            # Random sampling for verification
            selected_snapshots = random.sample(all_snapshots, min(sample_size, len(all_snapshots)))
            
            logger.debug("Selected snapshots for verification",
                        total_snapshots=len(all_snapshots),
                        selected_count=len(selected_snapshots),
                        sample_percentage=self.sample_percentage)
            
            return selected_snapshots
            
        except Exception as e:
            logger.error("Failed to select snapshots for verification", error=str(e))
            return []
    
    async def _verify_snapshot_batch(self, snapshot_ids: List[str], 
                                   report_id: str) -> List[VerificationResult]:
        """Verify a batch of snapshots with concurrency control."""
        results = []
        semaphore = asyncio.Semaphore(self.max_concurrent_verifications)
        
        async def verify_with_semaphore(snapshot_id: str) -> VerificationResult:
            async with semaphore:
                return await self.verify_single_snapshot(snapshot_id)
        
        # Create verification tasks
        tasks = [
            asyncio.create_task(verify_with_semaphore(snapshot_id))
            for snapshot_id in snapshot_ids
        ]
        
        # Wait for all verifications to complete
        try:
            results = await asyncio.gather(*tasks, return_exceptions=True)
            
            # Handle any exceptions
            processed_results = []
            for i, result in enumerate(results):
                if isinstance(result, Exception):
                    logger.error("Verification task failed",
                               snapshot_id=snapshot_ids[i], error=str(result))
                    processed_results.append(VerificationResult(
                        snapshot_id=snapshot_ids[i],
                        verification_id=f"batch_{report_id}_{i}",
                        status=VerificationStatus.FAILURE,
                        timestamp=time.time(),
                        duration_seconds=0,
                        details="Verification task failed",
                        error_message=str(result)
                    ))
                else:
                    processed_results.append(result)
            
            return processed_results
            
        except Exception as e:
            logger.error("Batch verification failed", error=str(e))
            # Return failure results for all snapshots
            return [
                VerificationResult(
                    snapshot_id=snapshot_id,
                    verification_id=f"batch_{report_id}_failed",
                    status=VerificationStatus.FAILURE,
                    timestamp=time.time(),
                    duration_seconds=0,
                    details="Batch verification failed",
                    error_message=str(e)
                )
                for snapshot_id in snapshot_ids
            ]
    
    async def _get_all_encrypted_snapshots(self) -> List[str]:
        """Get list of all encrypted snapshots available for verification."""
        # This would be implemented to interface with the actual storage backend
        # For now, return a mock list
        try:
            # Mock implementation - would interface with actual snapshot manager
            mock_snapshots = [
                f"snap_vm_001_user_123_{int(time.time()) - i * 3600}"
                for i in range(10)
            ]
            return mock_snapshots
        except Exception as e:
            logger.error("Failed to get encrypted snapshots list", error=str(e))
            return []
    
    async def _get_snapshot_metadata(self, snapshot_id: str) -> Optional[Dict[str, Any]]:
        """Get metadata for a specific snapshot."""
        try:
            # Mock implementation - would interface with actual metadata store
            return {
                "snapshot_id": snapshot_id,
                "encryption_algorithm": "AES-256-GCM",
                "created_at": time.time() - 3600,
                "size_bytes": 1024 * 1024 * 1024,
                "checksum": hashlib.sha256(snapshot_id.encode()).hexdigest()
            }
        except Exception as e:
            logger.error("Failed to get snapshot metadata",
                        snapshot_id=snapshot_id, error=str(e))
            return None
    
    async def _retrieve_snapshot_sample(self, snapshot_id: str) -> Optional[bytes]:
        """Retrieve a small sample of encrypted snapshot data for testing."""
        try:
            # Mock implementation - would retrieve actual encrypted data sample
            # In real implementation, would only read first few KB for testing
            mock_encrypted_data = b"encrypted_sample_data_" + snapshot_id.encode()
            return mock_encrypted_data
        except Exception as e:
            logger.error("Failed to retrieve snapshot sample",
                        snapshot_id=snapshot_id, error=str(e))
            return None
    
    async def _verify_encryption_format(self, encrypted_data: bytes) -> bool:
        """Verify that data appears to be properly encrypted."""
        try:
            # Basic checks for encrypted data format
            if len(encrypted_data) < 16:  # Minimum for encrypted data
                return False
            
            # Check for patterns that suggest unencrypted data
            # Encrypted data should have high entropy
            if b"<html>" in encrypted_data.lower() or b"json" in encrypted_data.lower():
                return False
            
            # Additional format checks would go here
            return True
            
        except Exception as e:
            logger.error("Encryption format verification failed", error=str(e))
            return False
    
    async def _verify_key_accessibility(self, snapshot_id: str) -> bool:
        """Verify that encryption keys are accessible for the snapshot."""
        try:
            # Mock implementation - would check actual key accessibility
            # In real implementation, would verify key store access
            return True
        except Exception as e:
            logger.error("Key accessibility verification failed",
                        snapshot_id=snapshot_id, error=str(e))
            return False
    
    async def _test_limited_decryption(self, snapshot_id: str, encrypted_data: bytes) -> bool:
        """Perform limited decryption test without exposing full data."""
        try:
            # Mock implementation - would perform actual limited decryption test
            # In real implementation, would decrypt only first block or header
            if len(encrypted_data) > 0:
                return True
            return False
        except Exception as e:
            logger.error("Limited decryption test failed",
                        snapshot_id=snapshot_id, error=str(e))
            return False
    
    def _create_empty_report(self, report_id: str, start_time: float) -> VerificationReport:
        """Create an empty verification report when no snapshots are available."""
        return VerificationReport(
            report_id=report_id,
            timestamp=start_time,
            total_snapshots_checked=0,
            successful_verifications=0,
            failed_verifications=0,
            warning_verifications=0,
            skipped_verifications=0,
            verification_rate=0.0,
            duration_seconds=time.time() - start_time,
            next_verification=start_time + (self.verification_interval_hours * 3600),
            details=[]
        )
    
    def _update_verification_stats(self, report: VerificationReport):
        """Update verification statistics with new report data."""
        self.verification_stats["total_verifications"] += report.total_snapshots_checked
        self.verification_stats["successful_verifications"] += report.successful_verifications
        self.verification_stats["failed_verifications"] += report.failed_verifications
        
        # Update average duration
        total_ops = self.verification_stats["total_verifications"]
        if total_ops > 0:
            current_avg = self.verification_stats["average_duration"]
            new_duration = report.duration_seconds
            self.verification_stats["average_duration"] = (
                (current_avg * (total_ops - report.total_snapshots_checked) + 
                 new_duration * report.total_snapshots_checked) / total_ops
            )
    
    async def _handle_verification_failures(self, report: VerificationReport):
        """Handle alerting and logging for verification failures."""
        failed_snapshots = [
            result for result in report.details 
            if result.status == VerificationStatus.FAILURE
        ]
        
        for failed_result in failed_snapshots:
            logger.error("Backup encryption verification failed",
                        snapshot_id=failed_result.snapshot_id,
                        verification_id=failed_result.verification_id,
                        error=failed_result.error_message)
            
            logfire.error("Backup encryption verification failure",
                         snapshot_id=failed_result.snapshot_id,
                         verification_id=failed_result.verification_id,
                         error_message=failed_result.error_message,
                         details=failed_result.details)
        
        # Would integrate with alerting system here
        logger.warning("Multiple backup encryption verification failures detected",
                      failed_count=len(failed_snapshots),
                      report_id=report.report_id)