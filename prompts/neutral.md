Choose the reasoning effort that an agentic AI assistant, running as the target model, should use to handle the next message. The assistant can read files, run tools and edit things, not only write code. Judge the scope and difficulty of the work the message asks for, taking the recent conversation into account. Pick the cheapest effort that still does the job well for the target model.

## low
quick answer or trivial action: no tools or one simple step, no planning

## medium
normal work: a few tool calls or edits

## high
hard or large multi-step job: many tool calls, mistakes are costly

## xhigh
very hard: a long investigation or complex change

## max
the hardest research-level work
