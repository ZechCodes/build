"""Advanced middleware for request/response processing."""

import gzip
import uuid
import time
from typing import Optional
from fastapi import Request, Response
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import StreamingResponse
import structlog

logger = structlog.get_logger(__name__)


class RequestIDMiddleware(BaseHTTPMiddleware):
    """Middleware to add unique request ID for tracing."""
    
    def __init__(self, app, header_name: str = "X-Request-ID"):
        super().__init__(app)
        self.header_name = header_name
    
    async def dispatch(self, request: Request, call_next):
        """Add request ID to request and response."""
        
        # Check if request ID already exists in headers
        request_id = request.headers.get(self.header_name)
        
        if not request_id:
            # Generate new request ID
            request_id = str(uuid.uuid4())
        
        # Add to request state
        request.state.request_id = request_id
        
        # Process request
        response = await call_next(request)
        
        # Add request ID to response headers
        response.headers[self.header_name] = request_id
        
        return response


class CompressionMiddleware(BaseHTTPMiddleware):
    """Middleware for response compression."""
    
    def __init__(
        self,
        app,
        minimum_size: int = 1024,
        compression_level: int = 6,
        compressible_types: Optional[set] = None
    ):
        super().__init__(app)
        self.minimum_size = minimum_size
        self.compression_level = compression_level
        self.compressible_types = compressible_types or {
            "application/json",
            "application/javascript",
            "text/html",
            "text/css",
            "text/plain",
            "text/xml",
            "application/xml"
        }
    
    async def dispatch(self, request: Request, call_next):
        """Compress response if applicable."""
        
        # Check if client accepts gzip
        accept_encoding = request.headers.get("accept-encoding", "")
        if "gzip" not in accept_encoding.lower():
            return await call_next(request)
        
        response = await call_next(request)
        
        # Check if response should be compressed
        if not self._should_compress(response):
            return response
        
        # Compress response
        return await self._compress_response(response)
    
    def _should_compress(self, response: Response) -> bool:
        """Determine if response should be compressed."""
        
        # Check content type
        content_type = response.headers.get("content-type", "")
        if not any(ct in content_type for ct in self.compressible_types):
            return False
        
        # Check if already compressed
        if response.headers.get("content-encoding"):
            return False
        
        # Check content length
        content_length = response.headers.get("content-length")
        if content_length and int(content_length) < self.minimum_size:
            return False
        
        return True
    
    async def _compress_response(self, response: Response) -> StreamingResponse:
        """Compress response content."""
        
        # Get response content
        if hasattr(response, 'body'):
            content = response.body
        else:
            # For streaming responses, we'd need different handling
            return response
        
        # Compress content
        compressed_content = gzip.compress(content, compresslevel=self.compression_level)
        
        # Update headers
        response.headers["content-encoding"] = "gzip"
        response.headers["content-length"] = str(len(compressed_content))
        response.headers["vary"] = "Accept-Encoding"
        
        # Create new response with compressed content
        return Response(
            content=compressed_content,
            status_code=response.status_code,
            headers=response.headers,
            media_type=response.media_type
        )


class SecurityHeadersAdvancedMiddleware(BaseHTTPMiddleware):
    """Advanced security headers middleware."""
    
    def __init__(self, app, config: Optional[dict] = None):
        super().__init__(app)
        self.config = config or {}
        
        # Default security headers
        self.default_headers = {
            "X-Content-Type-Options": "nosniff",
            "X-Frame-Options": "DENY",
            "X-XSS-Protection": "1; mode=block",
            "Referrer-Policy": "strict-origin-when-cross-origin",
            "Permissions-Policy": "geolocation=(), microphone=(), camera=()",
            "Cross-Origin-Embedder-Policy": "require-corp",
            "Cross-Origin-Opener-Policy": "same-origin",
            "Cross-Origin-Resource-Policy": "same-origin"
        }
        
        # HSTS configuration
        self.hsts_config = self.config.get("hsts", {
            "max_age": 31536000,  # 1 year
            "include_subdomains": True,
            "preload": True
        })
        
        # CSP configuration
        self.csp_config = self.config.get("csp", {
            "default_src": ["'self'"],
            "script_src": ["'self'", "'unsafe-inline'"],
            "style_src": ["'self'", "'unsafe-inline'"],
            "img_src": ["'self'", "data:", "https:"],
            "connect_src": ["'self'"],
            "font_src": ["'self'"],
            "object_src": ["'none'"],
            "media_src": ["'self'"],
            "frame_src": ["'none'"]
        })
    
    async def dispatch(self, request: Request, call_next):
        """Add security headers to response."""
        
        response = await call_next(request)
        
        # Add default security headers
        for header, value in self.default_headers.items():
            if header not in response.headers:
                response.headers[header] = value
        
        # Add HSTS header for HTTPS requests
        if request.url.scheme == "https":
            hsts_value = f"max-age={self.hsts_config['max_age']}"
            if self.hsts_config.get("include_subdomains"):
                hsts_value += "; includeSubDomains"
            if self.hsts_config.get("preload"):
                hsts_value += "; preload"
            response.headers["Strict-Transport-Security"] = hsts_value
        
        # Add CSP header
        csp_directives = []
        for directive, sources in self.csp_config.items():
            if sources:
                csp_directives.append(f"{directive.replace('_', '-')} {' '.join(sources)}")
        
        if csp_directives:
            response.headers["Content-Security-Policy"] = "; ".join(csp_directives)
        
        return response


class ProcessTimeMiddleware(BaseHTTPMiddleware):
    """Middleware to add process time header."""
    
    def __init__(self, app, header_name: str = "X-Process-Time"):
        super().__init__(app)
        self.header_name = header_name
    
    async def dispatch(self, request: Request, call_next):
        """Add process time to response headers."""
        
        start_time = time.time()
        response = await call_next(request)
        process_time = time.time() - start_time
        
        response.headers[self.header_name] = str(round(process_time, 4))
        
        return response


class RequestSizeLimitAdvancedMiddleware(BaseHTTPMiddleware):
    """Advanced request size limiting with detailed error responses."""
    
    def __init__(
        self,
        app,
        max_request_size: int = 50 * 1024 * 1024,  # 50MB
        max_json_size: int = 10 * 1024 * 1024,     # 10MB
        max_form_size: int = 25 * 1024 * 1024      # 25MB
    ):
        super().__init__(app)
        self.max_request_size = max_request_size
        self.max_json_size = max_json_size
        self.max_form_size = max_form_size
    
    async def dispatch(self, request: Request, call_next):
        """Check request size limits."""
        
        content_length = request.headers.get("content-length")
        if content_length:
            content_length = int(content_length)
            
            # Check overall request size
            if content_length > self.max_request_size:
                return Response(
                    content=f"Request too large. Maximum size: {self.max_request_size} bytes",
                    status_code=413,
                    headers={
                        "Content-Type": "text/plain",
                        "Retry-After": "3600"  # Suggest retry after 1 hour
                    }
                )
            
            # Check content-type specific limits
            content_type = request.headers.get("content-type", "")
            
            if "application/json" in content_type and content_length > self.max_json_size:
                return Response(
                    content=f"JSON payload too large. Maximum size: {self.max_json_size} bytes",
                    status_code=413,
                    headers={"Content-Type": "text/plain"}
                )
            
            if "multipart/form-data" in content_type and content_length > self.max_form_size:
                return Response(
                    content=f"Form data too large. Maximum size: {self.max_form_size} bytes",
                    status_code=413,
                    headers={"Content-Type": "text/plain"}
                )
        
        return await call_next(request)