"""Workflow engine — sequential multi-agent execution with Pydantic validation.

Inspired by the Antfarm pattern: each agent gets a fresh LLM context window,
structured output validation at every gate, retry with escalation on failure.
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from enum import Enum

logger = logging.getLogger(__name__)
from typing import Any, Callable

from pydantic import BaseModel

from openclaw.llm import LLMClient
from openclaw.notify import TelegramNotifier


class OnFail(str, Enum):
    RETRY = "retry"
    ESCALATE = "escalate"
    ABORT = "abort"


@dataclass
class StepDef:
    """One step in a workflow. Each step gets a fresh LLM context."""

    id: str
    agent: str
    input_schema: type[BaseModel]
    output_schema: type[BaseModel]
    validate: Callable[[BaseModel], bool]
    max_retries: int = 2
    on_fail: OnFail = OnFail.ESCALATE


@dataclass
class WorkflowDef:
    """A complete workflow definition."""

    id: str
    name: str
    steps: list[StepDef] = field(default_factory=list)


@dataclass
class StepResult:
    """Result of executing a single step."""

    step_id: str
    agent: str
    attempt: int
    output: BaseModel | None
    passed_gate: bool
    duration_ms: int
    error: str | None = None


@dataclass
class WorkflowResult:
    """Complete result of a workflow run, including all intermediate outputs."""

    final_output: BaseModel
    step_outputs: dict[str, BaseModel]  # step_id -> output
    run_id: int


class WorkflowEngine:
    """Execute workflows as sequential agent steps with gating."""

    def __init__(
        self,
        db: Any,
        llm: LLMClient,
        notifier: TelegramNotifier | None = None,
    ):
        self.db = db
        self.llm = llm
        self.notifier = notifier
        self.agents: dict[str, Any] = {}  # name -> BaseAgent

    def register_agent(self, name: str, agent: Any) -> None:
        self.agents[name] = agent

    async def run(
        self,
        workflow: WorkflowDef,
        initial_input: BaseModel,
    ) -> WorkflowResult | None:
        """Execute a workflow end-to-end. Returns WorkflowResult or None if aborted."""
        run_id = await self.db.create_workflow_run(
            workflow_id=workflow.id,
            trigger="manual",
            input_data=initial_input.model_dump(),
        )

        context: BaseModel = initial_input
        all_results: list[StepResult] = []

        for step in workflow.steps:
            agent = self.agents.get(step.agent)
            if agent is None:
                raise ValueError(f"Agent '{step.agent}' not registered")

            result = await self._execute_step(run_id, step, agent, context)
            all_results.append(result)

            if result.passed_gate and result.output is not None:
                context = result.output
            else:
                # Step failed after all retries
                if step.on_fail == OnFail.ESCALATE and self.notifier:
                    await self.notifier.escalate(
                        workflow_name=workflow.name,
                        step_id=step.id,
                        context=context.model_dump(),
                        error=result.error,
                    )
                await self.db.complete_workflow_run(run_id, status="failed")
                return None

        await self.db.complete_workflow_run(
            run_id,
            status="completed",
            result=context.model_dump(),
        )

        step_outputs = {
            r.step_id: r.output for r in all_results if r.output is not None
        }
        return WorkflowResult(
            final_output=context,
            step_outputs=step_outputs,
            run_id=run_id,
        )

    async def _execute_step(
        self,
        run_id: int,
        step: StepDef,
        agent: Any,
        context: BaseModel,
    ) -> StepResult:
        """Execute a single step with retries."""
        last_result = None

        for attempt in range(step.max_retries + 1):
            start = time.monotonic()
            error = None

            try:
                output = await agent.execute(
                    step_id=step.id,
                    input_data=context,
                    output_schema=step.output_schema,
                )
                passed = step.validate(output)
            except Exception as e:
                output = None
                passed = False
                error = str(e)
                logger.error("Step [%s] attempt %d error: %s", step.id, attempt, e, exc_info=True)

            duration_ms = int((time.monotonic() - start) * 1000)

            result = StepResult(
                step_id=step.id,
                agent=step.agent,
                attempt=attempt,
                output=output,
                passed_gate=passed,
                duration_ms=duration_ms,
                error=error,
            )

            await self.db.log_step(
                run_id=run_id,
                step_id=step.id,
                agent=step.agent,
                attempt=attempt,
                input_data=context.model_dump(),
                output_data=output.model_dump() if output else None,
                passed_gate=passed,
                duration_ms=duration_ms,
            )

            if passed:
                return result

            last_result = result

        return last_result or StepResult(
            step_id=step.id,
            agent=step.agent,
            attempt=step.max_retries,
            output=None,
            passed_gate=False,
            duration_ms=0,
            error="All retries exhausted",
        )
