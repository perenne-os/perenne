//! Scheduling: a fixed-size round-robin run queue plus the context
//! switch that makes it real.
//!
//! Pure here: the `Scheduler` struct and `pick_next` (host-tested). The
//! gated section below adds the static `SCHED` instance, the
//! `switch_context` assembly, `spawn`, `yield_now`, `enter`, and
//! `preempt` — everything that touches CSRs, assembly, or the live
//! console.

use crate::task::{Context, Task, TaskState};

/// Maximum concurrent tasks: the three demo tasks plus one slot of
/// headroom. The bootstrap (`kmain`) context is NOT a slot — it is a
/// throwaway `Context` used only for the first switch (see `enter`), so
/// the rotation is purely among the spawned tasks.
pub const MAX_TASKS: usize = 4;

/// The run queue: a fixed array of optional tasks and the index of the
/// one currently running.
pub struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],
    current: usize,
}

impl Scheduler {
    /// An empty scheduler; `spawn` fills slots and `enter` starts it.
    pub const fn new() -> Self {
        Self { tasks: [None, None, None, None], current: 0 }
    }

    /// Index of the next task to run after `current`, round-robin: scan
    /// forward (wrapping) for the next `Ready` slot, skipping empty
    /// slots and the current task. Returns `current` if nobody else is
    /// ready — the caller then keeps running.
    pub fn pick_next(&self) -> usize {
        for offset in 1..=MAX_TASKS {
            let i = (self.current + offset) % MAX_TASKS;
            if let Some(t) = &self.tasks[i] {
                if t.state == TaskState::Ready {
                    return i;
                }
            }
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &'static str, state: TaskState) -> Task {
        Task { context: Context::zeroed(), state, stack_top: 0, name }
    }

    /// Three ready tasks in slots 0..3, slot 3 empty; `current` running.
    fn three_tasks(current: usize) -> Scheduler {
        let mut s = Scheduler::new();
        s.tasks[0] = Some(task("A", TaskState::Ready));
        s.tasks[1] = Some(task("B", TaskState::Ready));
        s.tasks[2] = Some(task("C", TaskState::Ready));
        s.current = current;
        s.tasks[current].as_mut().unwrap().state = TaskState::Running;
        s
    }

    #[test]
    fn rotates_to_the_next_ready_task() {
        let s = three_tasks(0);
        assert_eq!(s.pick_next(), 1);
    }

    #[test]
    fn wraps_around_and_skips_empty_slots() {
        // current = 2; slot 3 is empty, so the next ready is slot 0.
        let s = three_tasks(2);
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn single_task_returns_itself() {
        let mut s = Scheduler::new();
        s.tasks[0] = Some(task("solo", TaskState::Running));
        s.current = 0;
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn no_other_ready_task_returns_current() {
        // All three present but the other two are Running (artificial) —
        // no Ready peer, so pick_next keeps the current task.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state = TaskState::Running;
        s.tasks[2].as_mut().unwrap().state = TaskState::Running;
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn full_rotation_visits_every_task_once() {
        let mut s = three_tasks(0);
        let mut order = alloc_order(&mut s);
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2]);
    }

    // Simulate three cooperative yields and record who runs.
    fn alloc_order(s: &mut Scheduler) -> [usize; 3] {
        let mut seen = [0usize; 3];
        for slot in seen.iter_mut() {
            let next = s.pick_next();
            s.tasks[s.current].as_mut().unwrap().state = TaskState::Ready;
            s.tasks[next].as_mut().unwrap().state = TaskState::Running;
            s.current = next;
            *slot = next;
        }
        seen
    }
}
