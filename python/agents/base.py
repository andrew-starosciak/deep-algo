"""Base agent class — wraps LLM call with role-specific prompt + data gathering."""

from __future__ import annotations

from pathlib import Path
from typing import Any, TypeVar

from pydantic import BaseModel

from openclaw.llm import LLMClient

T = TypeVar("T", bound=BaseModel)

PROMPTS_DIR = Path(__file__).parent.parent / "prompts"


class BaseAgent:
    """Each agent wraps an LLM call with role-specific prompt + optional data gathering.

    Subclasses override:
    - role: Agent's role description
    - prompt_file: Path to the prompt template markdown file
    - gather_context(): Fetch additional data before the LLM call
    """

    def __init__(self, llm: LLMClient, db: Any = None):
        self.llm = llm
        self.db = db

    @property
    def role(self) -> str:
        raise NotImplementedError

    @property
    def prompt_file(self) -> str:
        raise NotImplementedError

    def _load_prompt(self) -> str:
        path = PROMPTS_DIR / self.prompt_file
        if path.exists():
            return path.read_text()
        return f"You are a {self.role}. Analyze the input and produce structured output."

    async def gather_context(self, input_data: BaseModel) -> dict:
        """Override to fetch additional data from DB/APIs before LLM call."""
        return {}

    async def execute(
        self,
        step_id: str,
        input_data: BaseModel,
        output_schema: type[T],
    ) -> T:
        """Execute this agent: gather context, build prompt, call LLM."""
        extra_context = await self.gather_context(input_data)
        prompt_template = self._load_prompt()

        prompt = prompt_template.format(
            input=input_data.model_dump_json(indent=2),
            **extra_context,
        )

        return await self.llm.structured_output(
            system=f"You are a {self.role}.",
            prompt=prompt,
            response_model=output_schema,
        )
