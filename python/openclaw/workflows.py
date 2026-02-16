"""Workflow definitions — concrete step pipelines for each trading workflow."""

from __future__ import annotations

from openclaw.engine import OnFail, StepDef, WorkflowDef
from schemas.research import ResearchRequest, ResearchSummary
from schemas.risk import RiskVerification
from schemas.thesis import Thesis


def trade_thesis_workflow() -> WorkflowDef:
    """Research → Score thesis → Verify risk → Write recommendation."""
    return WorkflowDef(
        id="trade-thesis",
        name="Options Trade Thesis",
        steps=[
            StepDef(
                id="research",
                agent="researcher",
                input_schema=ResearchRequest,
                output_schema=ResearchSummary,
                validate=lambda r: r.opportunity_score >= 3,
                max_retries=1,
                on_fail=OnFail.ABORT,  # Not interesting → skip, don't escalate
            ),
            StepDef(
                id="evaluate",
                agent="analyst",
                input_schema=ResearchSummary,
                output_schema=Thesis,
                validate=lambda t: t.scores.overall >= 7.0,
                max_retries=1,
                on_fail=OnFail.ABORT,  # Doesn't meet threshold
            ),
            StepDef(
                id="verify",
                agent="risk_checker",
                input_schema=Thesis,
                output_schema=RiskVerification,
                validate=lambda r: r.approved and r.position_size_pct <= 2.0,
                max_retries=1,
                on_fail=OnFail.ESCALATE,  # Ping human
            ),
        ],
    )


WORKFLOWS: dict[str, WorkflowDef] = {
    "trade-thesis": trade_thesis_workflow(),
}


def get_workflow(name: str) -> WorkflowDef:
    """Look up a workflow by name."""
    wf = WORKFLOWS.get(name)
    if wf is None:
        available = ", ".join(WORKFLOWS.keys())
        raise ValueError(f"Unknown workflow '{name}'. Available: {available}")
    return wf
