"""An invitation's delivery and address-binding contract, independent of its state."""

from enum import StrEnum


class InviteKind(StrEnum):
    EMAIL_BOUND = "email_bound"
    OPEN_LINK = "open_link"
