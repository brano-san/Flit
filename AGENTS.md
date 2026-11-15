# PROJECT ENGINEERING GUIDELINES

## 1. PROJECT UNDERSTANDING

Before making changes, understand the existing application.

- Inspect the repository structure and relevant documentation.
- Identify the technology stack and framework versions.
- Understand frontend, backend, database and integrations.
- Inspect existing implementations similar to the requested feature.
- Identify shared components, utilities and design conventions.
- Check current Git changes before editing files.

Do not assume the project uses a particular architecture or library.
The actual codebase is the source of truth.

## 2. IMPLEMENTATION OWNERSHIP

My requests may be short and abstract. Treat them as requests to deliver the intended functionality. Independently determine affected files, placement, reuse, required layers and edge cases. Do not ask me to specify details discoverable from the project.

## 3. UI/UX AND DESIGN CONSISTENCY

The existing application defines its visual language. Inspect neighboring pages and similar components before UI changes. Reuse the established design system and follow its typography, colors, spacing, radii, iconography, visual hierarchy and responsive behavior. Place missing elements based on purpose, context and product conventions. Use relevant UI/UX skills when they materially help. Do not force a redesign when a focused improvement is sufficient.

## 4. FULLSTACK IMPLEMENTATION

A feature is not complete merely because its frontend exists. Consider frontend components, state, forms, validation, loading/success/empty/error states, responsiveness and accessibility; backend APIs, business logic, authentication, authorization, input validation, error handling and services; and database schema, migrations, indexes, constraints, integrity and compatibility. Implement every layer required by the outcome without modifying unrelated systems.

## 5. EXISTING ARCHITECTURE

- Follow established project patterns and reuse abstractions and utilities.
- Respect module boundaries and avoid duplicated business logic.
- Avoid unnecessary dependencies and competing design systems.
- Do not replace working architecture without a strong reason.
- Keep changes focused and maintainable.
- For React and Next.js projects, use relevant React best practices when available.

## 6. EXECUTION WORKFLOW

For substantial tasks: investigate, decide, plan only as needed, implement the complete functionality, integrate affected layers, verify behavior and visual output, fix discovered issues, and report the result. Do not stop after presenting a plan. For small tasks, execute directly without unnecessary planning.

## 7. VISUAL VERIFICATION

For UI changes, use available browser automation or visual tools. When supported, inspect the actual page, placement, alignment, spacing, interactions, responsive layouts and relevant states; fix visible problems and inspect again. Do not claim visual verification without performing it. If browser tools are unavailable, use the strongest available alternative and report the limitation.

## 8. TESTING

- Run relevant targeted tests first.
- Run type checking and linting when available.
- Run the production build when appropriate.
- Check important regression scenarios and actual user-facing functionality.
- Use project-defined scripts and conventions; do not invent commands.
- Run the full suite only when justified; do not repeat unchanged tests.
- Report exactly what was tested and never claim checks without evidence.

## 9. DOCUMENTATION AND PROJECT KNOWLEDGE

Use existing documentation before creating new documentation. Read only relevant documents. Update documentation when architecture, important decisions, public interfaces or project status materially change. Do not document every minor code change.

## 10. SAFETY

- Preserve unrelated local changes and never expose secrets or credentials.
- Never use fake data as a substitute for required functionality.
- Never silently remove existing features.
- Ask before destructive database operations, production deployments or irreversible changes.
- Preserve existing security and permission checks.
- Ask before large architectural changes; explain blockers instead of repeatedly retrying.

## 11. COMPLETION CRITERIA

A task is complete when the requested outcome is implemented, required layers are integrated, conventions are respected, relevant checks pass, UI changes are visually inspected when tools permit, and limitations are disclosed.

Final response in Russian:

1. What was implemented.
2. Where it was placed and why.
3. Important files changed.
4. Tests and verification performed.
5. Remaining limitations, if any.

## CODEX USAGE OPTIMIZATION

- Prefer minimal, targeted changes; avoid unnecessary refactoring.
- Avoid reading unrelated files or repeatedly inspecting files already understood.
- Reuse project patterns and utilities.
- Use the simplest approach that solves the task; do not over-engineer.
- Investigate only relevant dependencies and escalate complexity only when necessary.
- Do not duplicate work across agents; delegate only when parallel work provides real value.
- Ask before making large architectural changes.
- Summarize modified files and important decisions.

## FINAL RULE

Do not make me manage the development process. Understand my intent, explore the relevant project context, make reasonable decisions, implement and integrate the complete solution, verify it, fix issues and report completion.
