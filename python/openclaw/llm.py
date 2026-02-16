"""LLM client — structured output via Claude API + Instructor."""

from __future__ import annotations

import os
from typing import TypeVar

import anthropic
import instructor
from pydantic import BaseModel

T = TypeVar("T", bound=BaseModel)


class LLMClient:
    """Wrapper around Anthropic Claude API with structured output via Instructor."""

    def __init__(
        self,
        model: str = "claude-sonnet-4-5-20250929",
        api_key: str | None = None,
        max_tokens: int = 4096,
    ):
        self.model = model
        self.max_tokens = max_tokens
        raw_client = anthropic.Anthropic(
            api_key=api_key or os.environ.get("ANTHROPIC_API_KEY"),
        )
        self.client = instructor.from_anthropic(raw_client)

    async def structured_output(
        self,
        prompt: str,
        response_model: type[T],
        system: str | None = None,
    ) -> T:
        """Call the LLM and parse the response into a Pydantic model.

        Each call is a fresh context window — no conversation history carried over.
        This matches the Antfarm pattern of isolated agent contexts.
        """
        messages = [{"role": "user", "content": prompt}]

        response = self.client.messages.create(
            model=self.model,
            max_tokens=self.max_tokens,
            system=system or "You are a financial research analyst.",
            messages=messages,
            response_model=response_model,
        )

        return response

    async def text_output(
        self,
        prompt: str,
        system: str | None = None,
    ) -> str:
        """Call the LLM and return raw text (for summaries, battle plans, etc.)."""
        raw_client = anthropic.Anthropic(
            api_key=os.environ.get("ANTHROPIC_API_KEY"),
        )

        response = raw_client.messages.create(
            model=self.model,
            max_tokens=self.max_tokens,
            system=system or "You are a financial research analyst.",
            messages=[{"role": "user", "content": prompt}],
        )

        return response.content[0].text
