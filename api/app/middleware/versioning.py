"""API versioning and deprecation middleware."""

import re
from typing import Dict, Optional, Set
from datetime import datetime, timedelta
from fastapi import Request, Response
from starlette.middleware.base import BaseHTTPMiddleware
import structlog

logger = structlog.get_logger(__name__)


class APIVersioningMiddleware(BaseHTTPMiddleware):
    """Middleware for API versioning and deprecation management."""
    
    def __init__(
        self,
        app,
        supported_versions: Set[str] = None,
        default_version: str = "v1",
        deprecated_versions: Dict[str, datetime] = None,
        sunset_versions: Dict[str, datetime] = None
    ):
        super().__init__(app)
        self.supported_versions = supported_versions or {"v1"}
        self.default_version = default_version
        self.deprecated_versions = deprecated_versions or {}
        self.sunset_versions = sunset_versions or {}
    
    async def dispatch(self, request: Request, call_next):
        """Process request with version handling."""
        
        # Extract version from URL path or headers
        api_version = self._extract_version(request)
        
        # Validate version
        if api_version not in self.supported_versions:
            return Response(
                content=f"Unsupported API version: {api_version}. Supported versions: {', '.join(self.supported_versions)}",
                status_code=400,
                headers={"Content-Type": "text/plain"}
            )
        
        # Check for deprecated version
        deprecation_headers = {}
        if api_version in self.deprecated_versions:
            sunset_date = self.deprecated_versions[api_version]
            deprecation_headers.update({
                "Deprecation": sunset_date.strftime("%a, %d %b %Y %H:%M:%S GMT"),
                "Warning": f'299 - "API version {api_version} is deprecated and will be sunset on {sunset_date.strftime("%Y-%m-%d")}"'
            })
            
            logger.warning(
                "Deprecated API version used",
                version=api_version,
                path=request.url.path,
                user_agent=request.headers.get("user-agent"),
                sunset_date=sunset_date.isoformat()
            )
        
        # Check for sunset version
        if api_version in self.sunset_versions:
            sunset_date = self.sunset_versions[api_version]
            if datetime.utcnow() >= sunset_date:
                return Response(
                    content=f"API version {api_version} has been sunset as of {sunset_date.strftime('%Y-%m-%d')}",
                    status_code=410,  # Gone
                    headers={"Content-Type": "text/plain"}
                )
        
        # Add version to request state
        request.state.api_version = api_version
        
        # Process request
        response = await call_next(request)
        
        # Add version headers to response
        response.headers["API-Version"] = api_version
        response.headers["API-Supported-Versions"] = ", ".join(sorted(self.supported_versions))
        
        # Add deprecation headers if applicable
        for header, value in deprecation_headers.items():
            response.headers[header] = value
        
        return response
    
    def _extract_version(self, request: Request) -> str:
        """Extract API version from request."""
        
        # Try to extract from URL path (e.g., /api/v1/...)
        path_match = re.match(r"/api/(v\d+)/", request.url.path)
        if path_match:
            return path_match.group(1)
        
        # Try to extract from Accept header (e.g., application/vnd.buildplatform.v1+json)
        accept_header = request.headers.get("accept", "")
        accept_match = re.search(r"application/vnd\.buildplatform\.(v\d+)\+json", accept_header)
        if accept_match:
            return accept_match.group(1)
        
        # Try to extract from custom API-Version header
        version_header = request.headers.get("api-version")
        if version_header:
            return version_header
        
        # Default version
        return self.default_version


class APIDeprecationManager:
    """Manager for API deprecation and sunset policies."""
    
    def __init__(self):
        self.deprecation_schedule = {
            # Example: v1 will be deprecated 6 months from now, sunset 12 months from now
            # "v1": {
            #     "deprecated": datetime.utcnow() + timedelta(days=180),
            #     "sunset": datetime.utcnow() + timedelta(days=365)
            # }
        }
    
    def deprecate_version(
        self,
        version: str,
        deprecation_date: Optional[datetime] = None,
        sunset_date: Optional[datetime] = None
    ):
        """Mark a version as deprecated."""
        if deprecation_date is None:
            deprecation_date = datetime.utcnow()
        
        if sunset_date is None:
            sunset_date = deprecation_date + timedelta(days=180)  # 6 months default
        
        self.deprecation_schedule[version] = {
            "deprecated": deprecation_date,
            "sunset": sunset_date
        }
        
        logger.info(
            "API version deprecated",
            version=version,
            deprecation_date=deprecation_date.isoformat(),
            sunset_date=sunset_date.isoformat()
        )
    
    def get_deprecated_versions(self) -> Dict[str, datetime]:
        """Get currently deprecated versions."""
        now = datetime.utcnow()
        return {
            version: schedule["deprecated"]
            for version, schedule in self.deprecation_schedule.items()
            if schedule["deprecated"] <= now < schedule["sunset"]
        }
    
    def get_sunset_versions(self) -> Dict[str, datetime]:
        """Get sunset versions."""
        return {
            version: schedule["sunset"]
            for version, schedule in self.deprecation_schedule.items()
        }


# Global deprecation manager instance
deprecation_manager = APIDeprecationManager()


def create_versioning_middleware():
    """Create versioning middleware with current configuration."""
    return APIVersioningMiddleware(
        app=None,  # Will be set by FastAPI
        supported_versions={"v1", "v2"},  # Add v2 when ready
        default_version="v1",
        deprecated_versions=deprecation_manager.get_deprecated_versions(),
        sunset_versions=deprecation_manager.get_sunset_versions()
    )