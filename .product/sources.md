# Sources

Sources gathered on 2026-06-18.

## Market And Developer Population

1. GitHub Octoverse 2025  
   URL: https://github.blog/news-insights/octoverse/octoverse-a-new-developer-joins-github-every-second-as-ai-leads-typescript-to-1/  
   Useful facts: GitHub reported more than 180 million developers in 2025, 630 million projects, nearly 1 billion commits, and 43.2 million merged pull requests per month.  
   Caveat: GitHub developer accounts are not the same as active paid developer-tool buyers.

2. Stack Overflow Developer Survey 2025  
   URL: https://survey.stackoverflow.co/2025  
   Useful facts: Over 49,000 respondents from 177 countries. The technology section shows heavy modern toolchain usage, including Docker at 71.1% among all respondents and 73.8% among professional developers.  
   Caveat: Survey respondents are not a perfectly representative global developer population.

3. Mordor Intelligence: Software Development Tools Market  
   URL: https://www.mordorintelligence.com/industry-reports/software-development-tools-market  
   Useful facts: Estimates software development tools at USD 7.44B in 2026 and USD 15.72B in 2031.  
   Caveat: Analyst estimates are model-based and categories may not map directly to Bindhub.

4. Mordor Intelligence: Version Control System Market  
   URL: https://www.mordorintelligence.com/industry-reports/version-control-system-market  
   Useful facts: Estimates version control systems at USD 1.72B in 2026 and USD 3.74B in 2031. Notes cloud deployment and distributed VCS dominance.  
   Caveat: The product may expand beyond or sit adjacent to VCS budgets.

5. Grand View Research: Application Development Software Market  
   URL: https://www.grandviewresearch.com/industry-analysis/application-development-software-market-report  
   Useful facts: Estimates a much broader application development software market at USD 309.53B in 2025 and USD 862.67B by 2030.  
   Caveat: Too broad for direct TAM; useful as adjacency context only.

## Category And Competitive Context

6. Gartner Market Guide for Cloud Development Environments  
   URL: https://www.gartner.com/en/documents/5771915  
   Useful facts: Frames CDEs around developer productivity, onboarding, cloud-native complexity, AI engineering, and software supply-chain security.  
   Caveat: Full report may be gated; public abstract is enough for category context.

7. Gartner Peer Insights: Cloud Development Environments  
   URL: https://www.gartner.com/reviews/market/cloud-development-environments  
   Useful facts: Defines CDEs as remote, ready-to-use access to cloud-hosted development environments. Lists vendors such as GitHub Codespaces, Google Cloud Workstations, AWS Cloud9, and Coder.  
   Caveat: Peer Insights is review-oriented and not a market-size source.

8. Syncthing  
   URL: https://syncthing.net/  
   Useful facts: Continuous file synchronization between devices.  
   Caveat: Strong sync primitive, but not specifically a developer workspace timeline.

9. Mutagen  
   URL: https://mutagen.io/  
   Useful facts: Real-time file synchronization and network forwarding for developers using cloud containers and infrastructure.  
   Caveat: More focused on local-to-remote development than whole-code-folder continuity across personal machines.

10. Dropbox symlink documentation  
    URL: https://help.dropbox.com/sync/symlinks  
    Useful facts: Dropbox documents symlink and junction limitations, including Windows limitations and external symlink behavior.  
    Caveat: This is one example of why generic sync is not the same as dev-native sync.

11. Jujutsu VCS working copy docs  
    URL: https://docs.jj-vcs.dev/latest/working-copy/  
    Useful facts: Jujutsu automatically snapshots working-copy contents when commands run.  
    Caveat: Jujutsu improves source-control UX, but does not solve multi-machine workspace sync by itself.

12. Jujutsu GitHub repository  
    URL: https://github.com/jj-vcs/jj  
    Useful facts: Jujutsu has a production-ready Git backend and can work with Git remotes.  
    Caveat: Compatibility reinforces the strategy: improve the model without forcing immediate ecosystem replacement.

## AI And Agent Context

13. Sonar State of Code Developer Survey Report 2026  
    URL: https://www.sonarsource.com/state-of-code-developer-survey-report.pdf  
    Useful facts: Surveyed 1,149 professional developers; 72% of developers who have tried AI coding tools use them every day; 64% have started to use AI agents; 96% do not fully trust AI-generated code is functionally correct.  
    Caveat: The sample is professional developers who used AI as part of their job in the past year, so it over-represents AI users.

14. Theo Browne X thread, mirrored by Digg  
    URL: https://digg.com/tech/060emup5  
    Useful facts: Public developer discourse around Git not being the right primitive, commits/branches as weak UX, worktrees as painful, and source control needing better filesystem/API assumptions.  
    Caveat: This is opinion/context, not market evidence.

