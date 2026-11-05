Attached secrets (values are not included):
{descriptors}
Read a value with: buildctl --env {environment} secrets get --id <secret_id>
Project secrets are accessible to agents in this project. Agent secrets are limited to this agent’s conversation generation.
Printing, logging, or committing secret values is dangerous. A CLI read prints the value and may expose it in tool output, transcripts or terminal history. Retrieve only when needed and avoid unnecessary disclosure.
