# Market Sizing

## Scope

This sizing estimates the opportunity for a developer-native workspace continuity platform that begins with personal code-folder sync and expands into team workspaces, agent sandboxes, and Git-compatible source control.

This is not a mature standalone analyst category. The sizing triangulates from:

- developer population
- software development tools spend
- version control spend
- cloud development environment adoption
- AI coding and agent workflow adoption

## Source-Backed Anchors

- GitHub reported more than 180 million developers in 2025 and nearly 1 billion commits in 2025. This is the best broad proxy for the global developer universe.
- Mordor Intelligence estimates the software development tools market at USD 7.44B in 2026, growing to USD 15.72B by 2031.
- Mordor Intelligence estimates the version control system market at USD 1.72B in 2026, growing to USD 3.74B by 2031.
- Grand View Research estimates the broader application development software market at USD 309.53B in 2025 and USD 862.67B by 2030. This is too broad for direct TAM, but it confirms large budget gravity around app development.
- Gartner describes cloud development environments as a category created to improve developer productivity, onboarding, and supply-chain security amid AI and cloud-native complexity.
- Sonar's 2026 State of Code survey found that 64% of surveyed developers have started using AI agents and 72% of developers who have tried AI coding tools use them every day.

## Market Definition

### Narrow Market

Developer workspace continuity for local code folders, including sync, snapshots, restore, and Git-compatible work-in-progress preservation.

### Expansion Market

Workspace-state infrastructure for:

- team developer environments
- laptop replacement and onboarding
- secure code mobility
- agent sandboxes
- source-control timelines
- Git-compatible publishing

## Bottom-Up Sizing

The 180M GitHub developer figure includes students, hobbyists, inactive accounts, and non-paying users. The relevant paid population is smaller.

| Segment | Assumption | Seats | Annual price | Revenue pool |
| --- | ---: | ---: | ---: | ---: |
| Serious individual developers | 10% of GitHub developer base | 18M | USD 120 | USD 2.16B |
| Professional/team developers | 15% of GitHub developer base | 27M | USD 180 | USD 4.86B |
| Agent-enabled advanced seats | 5% of GitHub developer base | 9M | USD 300 | USD 2.70B |

These segments overlap, so they should not be summed mechanically. A practical long-term bottom-up TAM range is USD 3B to USD 9B, depending on whether the product remains a personal dev utility or becomes a workspace/source-control platform.

## Top-Down Cross-Check

The 2026 software development tools market plus version control market is roughly:

```text
USD 7.44B + USD 1.72B = USD 9.16B
```

That makes a USD 3B to USD 9B long-term TAM plausible if Devbox becomes a durable workspace layer that competes for developer tools, version-control, CDE, and agent-workflow budgets.

The broader application development software market is much larger, but it includes many products Devbox will not directly replace. It should be used as an adjacency signal, not as the core TAM.

## SAM And SOM

| Horizon | Definition | Seats | ARPA/year | Revenue |
| --- | --- | ---: | ---: | ---: |
| Initial SAM | GitHub-centric, multi-machine developers in US/EU/English-speaking markets | 5M to 10M | USD 120 to USD 180 | USD 600M to USD 1.8B |
| 5-year SOM, conservative | Paid personal and small-team adoption | 50k seats | USD 140 | USD 7M ARR |
| 5-year SOM, base | Strong solo wedge plus team preview | 250k seats | USD 180 | USD 45M ARR |
| 5-year SOM, upside | Team and agent workspace traction | 500k seats | USD 240 | USD 120M ARR |

## Sensitivity

The estimate is most sensitive to:

- paid-seat conversion from developer population
- whether teams buy this as a productivity/security layer
- whether agent workspaces become mandatory infrastructure
- whether the product can support enterprise governance without becoming a cloud IDE
- storage and sync costs per active project

## Pricing Hypothesis

- Free: one device, local snapshots, limited restore.
- Pro: USD 10/month for multi-device sync.
- Team: USD 15/month per developer for shared workspaces, policies, and admin.
- Business: USD 25/month per developer for SSO, audit, retention, managed secrets policy, device controls.
- Enterprise/Agent: custom pricing for large repos, compliance, air-gapped deployment, and agent sandbox fleets.

## Market Interpretation

The first market is not "everyone who uses Git." It is developers who feel acute pain from multi-machine work, WIP safety, local setup drift, and agent experimentation.

The bigger market opens only after the product proves it can preserve work safely. Trust is the conversion gate.

