# Devbox Product Folder

Last updated: 2026-06-19

This folder is the working product foundation for Devbox: developer folder continuity that can later
expand into teams, agent sandboxes, and a new source-control primitive.

## Core Thesis

Developers should be able to close a desktop, open a laptop, and continue in the same code folder with the same work-in-progress state, without pushing, pulling, stashing, zipping, rsyncing, or rebuilding context manually.

The wedge is simple:

> Your code folder, everywhere.

The foundation is deeper:

> Loom, a source-control primitive for file versions, folder revisions, checkpoints, safe sandboxes,
> shared overlays, and agent-friendly folder state.

## Folder Map

- [context.md](context.md) - business context, assumptions, and decision frame.
- [product-strategy.md](product-strategy.md) - product thesis, wedge, personas, principles, and positioning.
- [market-sizing.md](market-sizing.md) - TAM/SAM/SOM sizing, source-backed assumptions, and sensitivity.
- [kpi-framework.md](kpi-framework.md) - north star, activation, reliability, growth, and guardrail metrics.
- [product-business-analysis.md](product-business-analysis.md) - recommendation, opportunity logic, and prioritization.
- [architecture-foundation.md](architecture-foundation.md) - durable technical foundation for sync, Git compatibility, teams, and agents.
- [roadmap.md](roadmap.md) - phased plan from personal sync to Loom-powered source control.
- [go-to-market.md](go-to-market.md) - ICPs, positioning, pricing, and launch loops.
- [risk-register.md](risk-register.md) - product, technical, trust, security, and business risks.
- [experiments.md](experiments.md) - validation plan and user research scripts.
- [sources.md](sources.md) - public sources and caveats.
- [data/](data/) - structured assumptions, market sizing, KPI, and roadmap data.
- [templates/](templates/) - operating templates for review and research.
- [html/index.html](html/index.html) - browsable planning hub.

## Recommended First Decision

Start with solo developer, multi-machine folder sync. Do not start with teams, cloud IDEs, agents, or
Loom as the visible first public product.

That choice is not timid. It is the fastest way to earn the trust needed to own developer working
state. Once the product can safely sync a live code folder across machines, Loom can support team
sharing, agent sandboxes, human checkpoints, and Git-compatible publishing.

## Current Source Context

This pack uses public evidence and explicit assumptions. The Data Analytics user-context preflight found no saved source-routing preferences or semantic layers, so there are no internal dashboards, customer docs, or warehouse tables behind this version.

## Immediate Next Step

The private-alpha MVP control/safety surface now exists over the local-first foundations. The next
product step is alpha validation of one narrow promise:

1. Install on desktop.
2. Select a code folder.
3. Install on laptop.
4. Open the synced folder.
5. The exact working state is present, restorable, and safe.
