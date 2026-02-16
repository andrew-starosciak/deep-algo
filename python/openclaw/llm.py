"""LLM client using claude CLI (Claude Code subscription)."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import subprocess
from typing import TypeVar

from pydantic import BaseModel

T = TypeVar("T", bound=BaseModel)

logger = logging.getLogger(__name__)


class LLMClient:
    """LLM client using claude CLI with Claude Code subscription auth.

    Each call is a fresh context window — no conversation history carried over.
    Uses subprocess calls to `claude -p` instead of direct API calls.
    """

    def __init__(
        self,
        model: str = "claude-sonnet-4-5-20250929",
        api_key: str | None = None,
        max_tokens: int = 4096,
    ):
        self.model = model
        self.max_tokens = max_tokens
        # Note: claude CLI uses its own OAuth session, not ANTHROPIC_API_KEY
        # The api_key parameter is kept for backwards compatibility but not used
        self.api_key = api_key

    async def structured_output(
        self,
        prompt: str,
        response_model: type[T],
        system: str | None = None,
    ) -> T:
        """Call claude CLI and parse into a Pydantic model.

        Includes the JSON schema in the prompt to guide structured output.
        """
        # Include schema in prompt for structured output
        schema = response_model.model_json_schema()
        schema_str = json.dumps(schema, indent=2)

        full_prompt = f"""{prompt}

IMPORTANT: Respond with ONLY valid JSON (no markdown code fences, no extra text) that matches this exact schema:

{schema_str}

Your response must be parseable as JSON directly."""

        logger.debug(
            "LLM call: model=%s schema=%s prompt_len=%d",
            self.model, response_model.__name__, len(full_prompt),
        )

        text = await self._call_claude(full_prompt, system)

        # Extract JSON from response (handle markdown fences if present)
        json_str = self._extract_json(text)

        logger.info("LLM response: schema=%s json_len=%d", response_model.__name__, len(json_str))

        return response_model.model_validate_json(json_str)

    async def text_output(
        self,
        prompt: str,
        system: str | None = None,
    ) -> str:
        """Call claude CLI and return raw text."""
        return await self._call_claude(prompt, system)

    async def _call_claude(self, prompt: str, system: str | None = None) -> str:
        """Call claude CLI via subprocess.

        The claude CLI uses OAuth authentication stored by Claude Code,
        not ANTHROPIC_API_KEY environment variable.
        """

        def _run():
            cmd = [
                "claude",
                "-p",
                "--output-format", "json",
                "--model", self.model,
            ]
            logger.debug("Running command: %s", ' '.join(cmd))

            if system:
                # Prepend system message to prompt (claude CLI doesn't have --system flag)
                full_prompt = f"<system>{system}</system>\n\n{prompt}"
            else:
                full_prompt = prompt

            result = subprocess.run(
                cmd,
                input=full_prompt,
                capture_output=True,
                text=True,
                timeout=120,
            )

            if result.returncode != 0:
                logger.error("claude CLI failed: returncode=%d stdout=%s stderr=%s",
                             result.returncode, result.stdout[:500], result.stderr[:500])
                raise RuntimeError(
                    f"claude CLI failed (rc={result.returncode}): "
                    f"stderr={result.stderr[:200]} stdout={result.stdout[:200]}"
                )

            # Parse JSON envelope from claude CLI
            try:
                data = json.loads(result.stdout)
            except json.JSONDecodeError as e:
                logger.error("Failed to parse claude CLI output: %s", result.stdout[:200])
                raise RuntimeError(f"Invalid JSON from claude CLI: {e}")

            if data.get("is_error"):
                error_msg = data.get("result", "Unknown error")
                raise RuntimeError(f"claude CLI error: {error_msg}")

            return data.get("result", "")

        return await asyncio.get_event_loop().run_in_executor(None, _run)

    def _extract_json(self, text: str) -> str:
        """Extract JSON from text, handling markdown code fences."""
        text = text.strip()

        # Remove markdown fences if present
        if text.startswith("```"):
            # Find the content between fences
            match = re.search(r'```(?:json)?\s*\n(.*?)\n```', text, re.DOTALL)
            if match:
                return match.group(1).strip()
            # Fallback: strip first and last lines
            lines = text.split('\n')
            if len(lines) >= 3:
                return '\n'.join(lines[1:-1]).strip()

        return text
