# Reading the results

A run answers three questions: how long do cases take, where do they lose that time, and
which people (resource pools) hold them up. The Results tab shows the replication selected
at the top. `sim-cli run` prints the same findings after the figures of each replication.

## Findings

Each tab starts with findings in plain language, worst first. They come from
`sim_core::insights` and use these rules:

| Finding | When |
|---|---|
| **Problem:** the process does not keep up | The latest third of the completed cases (in order of arrival) took on average more than 1.5 times as long as the earliest third, with at least 30 completed cases. Queues grow for as long as cases arrive, so the cycle time depends on how long you simulate and never settles. Fix it before you read anything else. |
| **Problem:** failed cases or stuck tokens | See the Problems tab. |
| **Warning:** cases still open at the horizon | The run stopped at its time limit while cases were still open. Their cycle times are not in the statistics, and they are usually the long ones. |
| **Warning:** cases spend most of their time waiting | Waiting for resources is more than half of the average cycle time. More people where the cases wait helps more than faster work. |
| Pool *n%* busy (high / overloaded) | Utilisation of at least 80% (high) or 95% (overloaded). |
| Bottleneck | The pool whose tasks waited longest in total, if that waiting adds up to at least 5% of the total cycle time of all cases. |
| Pool busy only *n%* | Under 30% busy with more than one person, and one person less would stay under 80%. |

"With one more / less" scales the utilisation: the same work spread over a different number
of people. For an overloaded pool it is a lower bound, because the pool could not do all the
work it was given. Waiting times do not scale like this. Near full load, one more person
often removes most of the waiting. Run the scenario again with the new number to see by how
much.

## Cases

- **Where the time of an average case goes:** the cycle time of the completed cases (after the
  warm-up) split into *worked on* (working time of its tasks), *waiting for a resource* (from
  reaching a task until it started: waiting for a free person, or for working hours to
  begin), *paused outside working hours* (a started task interrupted by the end of the working
  day) and *other* (timers, messages, other pools, tasks without a resource). When tasks of a
  case run in parallel, the parts add up to more than the cycle time, and *other* is not
  shown.
- **Cycle time** and its histogram: the median is a typical case, p90 what 9 in 10 cases
  manage. A long tail to the right means a few cases wait much longer than the rest.
- **Cases in progress over time:** how many cases were open at once (the highest value in each
  of 200 equal stretches of the run). It should go up at the start and then level off. If it
  keeps rising while cases arrive, the process does not keep up. When a run ends after a
  number of cases, the curve falls at the end as the last cases are finished.
- **Individual cases:** the slowest, longest-waiting or most expensive cases (or all of them in
  order of arrival), with the task where each waited longest. Click a case to list its tasks:
  when it reached each one, how long it waited, worked, and what it cost.

## Resources

- **Utilisation:** busy time divided by the working time available (people × the pool's
  working hours during the run, warm-up included). Queueing theory says waiting rises steeply
  above about 80%: one person busy 80% of the time makes a task wait 4 task durations on
  average, at 90% 9 (M/M/1).
- **Waited (mean, p90):** how long the pool's completed tasks waited before they started.
- **Queue (mean, max):** how many tasks waited for the pool at once. The mean is the total
  waiting time divided by the length of the run (Little's law).
- **Tasks waiting over time:** the queue of each pool over the run. A queue that keeps rising
  belongs to a pool that cannot keep up.
- **What each pool works on:** the tasks that use the pool, with their runs, mean waiting time
  and mean working time. The bottleneck is opened.

The diagram's heat map shows the same waiting times and utilisation on the model.
