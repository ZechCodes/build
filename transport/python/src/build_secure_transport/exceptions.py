class SecureTransportError(Exception):
    """Base exception for secure transport protocol failures."""


class InvalidInputError(SecureTransportError):
    """Raised when caller input or envelope shape is invalid."""


class VerificationError(SecureTransportError):
    """Raised when signatures or pinned keys fail verification."""


class DecryptionError(SecureTransportError):
    """Raised when authenticated decryption fails."""


class ProtocolError(SecureTransportError):
    """Raised when a decrypted payload violates protocol rules."""
