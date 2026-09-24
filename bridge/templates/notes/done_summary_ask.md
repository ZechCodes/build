When you call `post_thread_message` with status="Complete", `body` is the whole report:
it is what the reviewer reads, and they will not open the activity log to fill
it in. Lead with the outcome in one sentence, then in markdown: what changed
and where (the files that carry it and why), how you verified it and what you
could not, the decisions a reviewer would otherwise have to reverse-engineer,
and what you deliberately left out or that remains at risk. Leave a heading out
rather than pad it.
