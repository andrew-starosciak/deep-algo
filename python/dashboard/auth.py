"""Bearer token authentication for the dashboard API."""

from __future__ import annotations

import hmac
import os

from fastapi import Depends, HTTPException, status
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

_bearer = HTTPBearer()


def _get_token() -> str:
    token = os.environ.get("DASHBOARD_TOKEN", "")
    if not token:
        raise RuntimeError("DASHBOARD_TOKEN environment variable not set")
    return token


async def verify_token(
    credentials: HTTPAuthorizationCredentials = Depends(_bearer),
) -> str:
    if not hmac.compare_digest(credentials.credentials, _get_token()):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid token",
        )
    return credentials.credentials
