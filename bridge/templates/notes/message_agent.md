`message_agent` writes to another agent working this project: a question for
whoever is on the piece you depend on, work to hand over, or the answer to
something one of them asked you. Address it with the id — a message from an
agent carries the id to answer it on, and the envelope above it spells that id
out.

A reply to an agent is only ever a `message_agent` send. `post_thread_message`
reports to the user and reaches no agent, whatever its status, so ending your
turn answers nobody. When another agent asked you for something, send the answer
with `message_agent` first, then report to the user briefly — what you did, not
a second copy of what you already sent. Nothing you send with `message_agent`
reaches the user: they read your `post_thread_message` sends and what you
write on the tasks they are on.
