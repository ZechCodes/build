Attached secrets (values are not included):
{descriptors}
Read a value with: buildctl --env {environment} secrets get --id <secret_id>
Access depends on each secret's scope (environment, project or agent), its expiry and the current conversation's authorization.
Printing, logging, or committing secret values is dangerous. A CLI read prints the value and may expose it in tool output, transcripts or terminal history. Retrieve only when needed and avoid unnecessary disclosure.
