Choose the reasoning effort an agentic assistant should use for the next user message, once for each model asked. It reads files, runs tools and edits things, not only code. Pick the cheapest effort that still does the job well.

Judge the scope and the amount of work, not whether it is code. A short message can ask for a lot: "go through my whole drive and clean it up", "review the entire repo", "migrate everything" are big, multi-step, tool-heavy jobs where mistakes are costly. Low is only for answers that need no tools and no planning.

If the message answers a question in the assistant's last reply (picks an option, says which one, confirms a plan), judge only the work that answer starts, as the reply describes it, not the length of the answer and not the work done before the question: "B" can mean "build the complicated section B" (high), while "yes" or "no" to one small action ("should I archive this?", "delete the old ones too?") is low, and picking a value for one setting (a log level, a colour, a font) is low. But picking which way to build something ("option 1", "the same shapes", "B") starts that build: judge the build. Approving a whole plan or several steps takes the effort of that plan. A plain "yes", "ok" or "do it" after "Is that OK? Then I'll build X" or "Should I build X?" approves building X: judge X, not the word.

A message that pushes back on, corrects or adds to a plan or claim under discussion continues that work: judge the effort that work needs, never low. Thanks, praise or a closing remark with no new request is low. A question about how to do something, or about effort itself, that needs no tools is low.

If the person asks for deep thought ("think hard", "ultrathink", "be thorough"), pick at least high; if they ask for a quick answer, pick low.

Effort levels mean a different amount of thinking on each model, so answer for each model on its own scale: the same task can need different levels on different models.

## haiku
Which effort should Claude Haiku 5.5, the fastest and cheapest: fine for small, well-defined work, strained by hard multi-step work use for the next message?

- low: chat, quick answers, short one-step tool tasks
- medium: ordinary work: a few tool calls or edits, normal coding
- high: longer multi-step tasks, multi-file changes, strict instructions
- xhigh: hard debugging, large refactors
- max: the hardest jobs it can try: migrations, architecture, long investigations

## sonnet
Which effort should Claude Sonnet 5.5, strong at everyday coding and agent work use for the next message?

- low: chat, writing, extraction, search; no planning
- medium: ordinary coding and multi-step tool work
- high: hard debugging, large refactors, careful reviews
- xhigh: very hard: long investigations, big migrations
- max: research-level work where a miss is very costly

## opus
Which effort should Claude Opus 5.5, the strongest of the three: hard debugging and architecture need less effort from it use for the next message?

- low: quick answers, small edits
- medium: most coding and debugging, multi-step work
- high: hard debugging, architecture, large refactors, migrations
- xhigh: very hard, long-horizon investigations
- max: the hardest research-level work

## fable
Which effort should Claude Fable 5.1, the most capable model, recommended at high for substantial work use for the next message?

- low: chat, quick answers, small edits
- medium: ordinary coding and short multi-step work
- high: most substantial work: hard debugging, refactors, migrations, architecture
- xhigh: very hard, long-horizon investigations
- max: the hardest research-level work where a miss is very costly
