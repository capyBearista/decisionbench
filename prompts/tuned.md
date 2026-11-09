Choose the reasoning effort that an agentic AI assistant should use to handle the next message, for each model asked about. The assistant can read files, run tools and edit things, not only write code. Judge the scope and difficulty of the work the message asks for, taking the recent conversation into account. Pick the cheapest effort that still does the job well for that model.

## haiku
Which effort should Claude Haiku 5.5 use for the next message? Haiku is the smallest model, so it needs more effort than a larger model for the same work: the same as the work alone suggests for a quick answer or trivial step, and one level more for any real task such as a fix, a feature, an analysis or a plan.

- low: quick answer or trivial action: no tools or one simple step, no planning
- medium: normal work: a few tool calls or edits
- high: a normal task: a fix, a feature, an analysis or a plan
- xhigh: very hard: a long investigation or complex change
- max: the hardest research-level work

## sonnet
Which effort should Claude Sonnet 5.5 use for the next message?

- low: quick answer or trivial action: no tools or one simple step, no planning
- medium: normal work: a few tool calls or edits
- high: hard or large multi-step job: many tool calls, mistakes are costly
- xhigh: very hard: a long investigation or complex change
- max: the hardest research-level work

## opus
Which effort should Claude Opus 5.5 use for the next message? Opus is a strong model, so it needs less effort than a smaller model for the same work: one level lower than the work alone suggests. Ordinary focused work (an explanation, a contained fix, a small feature) is low for Opus.

- low: quick answer or trivial action: no tools or one simple step, no planning
- medium: normal work: a few tool calls or edits
- high: hard or large multi-step job: many tool calls, mistakes are costly
- xhigh: very hard: a long investigation or complex change
- max: the hardest research-level work

## fable
Which effort should Claude Fable 5.1 use for the next message? Fable is the strongest model, so it needs the least effort for the same work: one or two levels lower than the work alone suggests. Ordinary focused work (an explanation, a contained fix, a small feature) is low for Fable.

- low: a quick answer or everyday work: an explanation, a contained fix, a small feature
- medium: normal work: a few tool calls or edits
- high: hard or large multi-step job: many tool calls, mistakes are costly
- xhigh: very hard: a long investigation or complex change
- max: the hardest research-level work
