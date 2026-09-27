The reviewer left comments on the plan document for stage "{stage_title}" at
{stage_path}, under Build's scratch docs directory for this task:

{docs_dir}

{comments}

Revise that stage document to address every comment. You may also update this
stage's "title" and "summary" fields in `.build/plan/stages.json`, but do not
add, remove, reorder, or re-id stages, and do not touch other stages' documents.
Keep writing only inside that docs directory — the primary checkout you are
running in stays untouched. When done, call `post_thread_message` with
status="Complete" and a concise report in body stating what was revised and
how you addressed each comment.
