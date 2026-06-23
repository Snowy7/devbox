# Business Context

## Decision Frame

We are planning a product that starts as developer folder continuity and can eventually become a
workspace platform for teams, agents, and source control.

The immediate decision is what foundation to build so the first wedge is small enough to ship, but the architecture does not block the later ambition.

## Working Definition

Bindhub is a developer-native folder continuity system.

It syncs developer folders across machines, but it does not behave like generic file sync. It
understands the developer-shaped mess inside those folders: repos, nested apps, ignored files,
generated artifacts, secrets, dependencies, restore points, and divergent work.

## What The User Has Made Clear

- The first felt need is personal and visceral: open a laptop and the code folder from the desktop is already there.
- The product should make folders feel like they are "at the same place" across machines.
- Existing products do not feel built for this developer workflow.
- The long-term ambition includes teams, agents, and a new source-control primitive codenamed Loom.
- Because of that ambition, the foundation matters more than a quick sync hack.

## Relevant Public Context

- GitHub reported more than 180 million developers on GitHub in 2025, plus nearly 1 billion commits and 43.2 million merged pull requests per month. This indicates both a huge developer population and an expanding volume of code-change activity.
- Stack Overflow's 2025 survey shows modern development is dependency-heavy and cloud/toolchain-heavy: Docker usage reached 71.1% among all respondents and 73.8% among professional developers. This reinforces why generic file sync struggles with code folders.
- Gartner defines cloud development environments as remote, ready-to-use cloud-hosted workspaces that reduce local setup friction. That validates the pain, but CDEs solve it by moving work away from the local machine. Bindhub should solve a different job: keep local workspaces continuous across machines.
- Jujutsu shows a credible direction for better source-control UX through automatic snapshots and
  Git compatibility. Bindhub should learn from this, but users should not need to replace Git on day
  one.
- Sonar's 2026 State of Code report says 64% of surveyed developers have started using AI agents and 72% of developers who have tried AI coding tools use them every day. Agent workspaces are not a side quest; they are a future workspace primitive.

## Key Assumptions

- The first buyer is an individual developer with more than one machine, not a platform team.
- Trust is the main adoption constraint. Losing work once is fatal.
- Git remains the collaboration standard for the first several years and must be supported as folder
  context.
- The product must preserve normal local workflows: terminal, IDE, Git CLI, language servers, file watchers, and local builds.
- The long-term platform should own folder state, not just files.

## Business Context Note

The market does not have a clean public category called "developer workspace sync." The opportunity sits between:

- generic file sync
- Git and Git hosting
- cloud development environments
- DevOps/developer productivity platforms
- AI agent sandbox orchestration

That ambiguity is good if the wedge is crisp. It is dangerous if positioning gets vague.

## Strongest Framing

Do not say:

> We are replacing Git.

Say:

> We make your code folder follow you everywhere, safely.

Then the product earns the right to say:

> Your folder already has a timeline. Git is just one way to publish it.
