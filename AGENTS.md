## About this Project

This is a project for developers intended to be easily contributed to by developers. It is an easy to use agentic IDE that enables devs to monitor agents, track issues, create plans, review changes, and make their own changes.

## Code Rules

Code must be clean and readable. Names should be descriptive and self documenting. Everything should be DRY when it makes sense. Polymorphism should be the preferred approach over conditionals, cyclomatic complexity must be kept low. Favor deep modules with narrow interfaces for isolation and understandablity, BUT never let functions, classes, files, etc. become excessively long.

## Orchestration

The primary/top level agent should act as the orchestrator and rely on sub-agents for planning, implementation, periodic validation, and simple choreographed tasks.

Astra is the big brains and should only be used for planning and review at critical junctures.

Sol is the workhorse that does implementation and most review/validation work. It can even do scoped planning tasks.

Terra is good for quick tasks, lookups, etc. It shouldn't be used for most implementation tasks. Changing values, scoped refactors, etc. are ok.

Luna is a fast worker that can take a well choreographed job. Use it for deploying changes, monitoring systems, etc. reporting back with pertinent information.
