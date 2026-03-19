//! Priority-based task queue for tile generation
//!
//! Manages tile generation tasks with three priority levels (High, Normal, Low).
//! Provides thread-safe concurrent access for multiple workers.

use crate::types::{Priority, TileCoord, TileTask};
use crate::tile_iterator::TileIterator;
use parking_lot::RwLock;
use std::collections::{VecDeque, HashMap, HashSet};
use std::sync::Arc;

/// Thread-safe priority-based task queue
///
/// Maintains three separate queues (one per priority level) and provides
/// thread-safe operations for enqueueing, dequeueing, and updating task priorities.
/// Also tracks dependencies between pyramid tiles and their children.
/// Low-priority tiles are generated via an iterator instead of a queue.
#[derive(Clone)]
pub struct TaskQueue {
    /// High priority queue
    high_queue: Arc<RwLock<VecDeque<TileTask>>>,
    /// Normal priority queue
    normal_queue: Arc<RwLock<VecDeque<TileTask>>>,
    /// Low priority queue (deprecated - use iterator instead)
    low_queue: Arc<RwLock<VecDeque<TileTask>>>,
    /// Iterator for generating low-priority background tiles
    low_priority_iterator: Arc<RwLock<Option<TileIterator>>>,
    /// Dependency tracking: child tile -> set of (parent tile, original priority)
    waiting_parents: Arc<RwLock<HashMap<TileCoord, HashSet<(TileCoord, Priority)>>>>,
}

impl TaskQueue {
    /// Create a new empty task queue
    pub fn new() -> Self {
        TaskQueue {
            high_queue: Arc::new(RwLock::new(VecDeque::new())),
            normal_queue: Arc::new(RwLock::new(VecDeque::new())),
            low_queue: Arc::new(RwLock::new(VecDeque::new())),
            low_priority_iterator: Arc::new(RwLock::new(None)),
            waiting_parents: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Initialize the low-priority tile iterator
    ///
    /// Should be called once after creating the queue with metadata available
    pub fn init_low_priority_iterator(&self, metadata: crate::types::FileMetadata) {
        let iterator = TileIterator::new(metadata);
        *self.low_priority_iterator.write() = Some(iterator);
    }
    
    /// Get the next low-priority tile from the iterator
    ///
    /// Returns None if iterator is not initialized or all tiles have been generated
    pub fn get_next_low_priority_tile(&self) -> Option<TileCoord> {
        let mut iterator_opt = self.low_priority_iterator.write();
        if let Some(ref mut iterator) = *iterator_opt {
            iterator.next()
        } else {
            None
        }
    }

    /// Enqueue a task into the appropriate priority queue
    ///
    /// Thread-safe insertion of a task based on its priority level.
    /// All queues use LIFO (most recent first) to prioritize current viewport requests.
    /// Queues are independent - a tile can exist in multiple queues simultaneously.
    pub fn enqueue(&self, task: TileTask) {
        match task.priority {
            Priority::High => {
                // LIFO for high priority: push to front so most recent requests are processed first
                self.high_queue.write().push_front(task);
            }
            Priority::Normal => {
                // LIFO for normal priority: push to front so most recent requests are processed first
                self.normal_queue.write().push_front(task);
            }
            Priority::Low => {
                // LIFO for low priority: push to front so most recent requests are processed first
                self.low_queue.write().push_front(task);
            }
        }
    }

    /// Dequeue the highest priority task
    ///
    /// Returns the next task to process, prioritizing high > normal > low.
    /// Returns None if all queues are empty.
    pub fn dequeue(&self) -> Option<TileTask> {
        // Try high priority first
        if let Some(task) = self.high_queue.write().pop_front() {
            return Some(task);
        }

        // Try normal priority
        if let Some(task) = self.normal_queue.write().pop_front() {
            return Some(task);
        }

        // Try low priority queue (for backward compatibility)
        if let Some(task) = self.low_queue.write().pop_front() {
            return Some(task);
        }
        
        // Try low priority iterator (generates background tiles on-demand)
        if let Some(coord) = self.get_next_low_priority_tile() {
            return Some(TileTask::new(coord, Priority::Low, coord.level == 0));
        }

        None
    }

    /// Update the priority of an existing task
    ///
    /// Finds the task with the given coordinate and updates its priority.
    /// If the task is not found, does nothing.
    pub fn update_priority(&self, coord: TileCoord, new_priority: Priority) {
        // Try to find and remove from current queue
        let old_priority = self.find_and_remove(coord);

        if let Some(mut task) = old_priority {
            task.priority = new_priority;
            self.enqueue(task);
        }
    }

    /// Remove a task from the queue
    ///
    /// Searches all queues for a task with the given coordinate and removes it.
    /// Returns the removed task if found, None otherwise.
    pub fn remove(&self, coord: TileCoord) -> Option<TileTask> {
        self.find_and_remove(coord)
    }

    /// Remove a tile from all queues (high, normal, and low priority)
    ///
    /// This is called when a tile has been successfully rendered and cached.
    /// The tile should be removed from all queues since it's no longer needed.
    pub fn remove_from_all_queues(&self, coord: TileCoord) {
        // Remove from high priority queue
        {
            let mut queue = self.high_queue.write();
            if let Some(pos) = queue.iter().position(|t| t.coord == coord) {
                queue.remove(pos);
            }
        }

        // Remove from normal priority queue
        {
            let mut queue = self.normal_queue.write();
            if let Some(pos) = queue.iter().position(|t| t.coord == coord) {
                queue.remove(pos);
            }
        }

        // Remove from low priority queue
        {
            let mut queue = self.low_queue.write();
            if let Some(pos) = queue.iter().position(|t| t.coord == coord) {
                queue.remove(pos);
            }
        }
    }

    /// Get the total number of tasks in all queues
    pub fn size(&self) -> usize {
        let high_count = self.high_queue.read().len();
        let normal_count = self.normal_queue.read().len();
        let low_count = self.low_queue.read().len();
        high_count + normal_count + low_count
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.size() == 0
    }

    /// Check if a task with the given coordinate exists in any queue
    pub fn contains(&self, coord: TileCoord) -> bool {
        // Check high priority queue
        if self.high_queue.read().iter().any(|t| t.coord == coord) {
            return true;
        }

        // Check normal priority queue
        if self.normal_queue.read().iter().any(|t| t.coord == coord) {
            return true;
        }

        // Check low priority queue
        if self.low_queue.read().iter().any(|t| t.coord == coord) {
            return true;
        }

        false
    }

    /// Clear all tasks from the high priority queue
    ///
    /// This is useful when the viewport changes (pan, zoom, resize) to discard
    /// tiles that are no longer needed and make room for new high-priority tiles
    pub fn clear_high_priority(&self) {
        let mut high = self.high_queue.write();
        let mut normal = self.normal_queue.write();
        
        for mut task in high.drain(..) {
            task.priority = Priority::Normal;
            normal.push_back(task);
        }
        
        log::debug!("High priority queue downgraded to normal to preserve dependencies");
    }

    /// Register that a parent tile is waiting for child tiles to complete
    ///
    /// When any of the child tiles complete, the parent will be automatically enqueued
    /// with the same priority it had when it registered the dependency
    pub fn register_waiting_parent(&self, parent: TileCoord, priority: Priority, children: &[TileCoord]) {
        let mut waiting = self.waiting_parents.write();
        for child in children {
            waiting.entry(*child).or_insert_with(HashSet::new).insert((parent, priority));
        }
        log::debug!(
            "Registered parent {:?} (priority {:?}) waiting for {} children",
            parent,
            priority,
            children.len()
        );
    }

    /// Notify that a tile has completed, and enqueue any parent tiles that were waiting for it
    ///
    /// Returns the list of parent tiles that were enqueued
    pub fn notify_tile_complete(&self, completed: TileCoord) -> Vec<TileCoord> {
        let mut waiting = self.waiting_parents.write();
        
        if let Some(parents) = waiting.remove(&completed) {
            let parent_list: Vec<_> = parents.iter().map(|(coord, _)| *coord).collect();
            
            log::debug!(
                "Tile {:?} completed, enqueueing {} waiting parents",
                completed,
                parent_list.len()
            );
            
            // Enqueue all waiting parents with their original priority
            for (parent, priority) in parents {
                self.enqueue(TileTask::new(parent, priority, false));
            }
            
            parent_list
        } else {
            Vec::new()
        }
    }


    /// Helper function to find and remove a task by coordinate
    fn find_and_remove(&self, coord: TileCoord) -> Option<TileTask> {
        // Try high priority queue
        {
            let mut queue = self.high_queue.write();
            if let Some(pos) = queue.iter().position(|t| t.coord == coord) {
                return queue.remove(pos);
            }
        }

        // Try normal priority queue
        {
            let mut queue = self.normal_queue.write();
            if let Some(pos) = queue.iter().position(|t| t.coord == coord) {
                return queue.remove(pos);
            }
        }

        // Try low priority queue
        {
            let mut queue = self.low_queue.write();
            if let Some(pos) = queue.iter().position(|t| t.coord == coord) {
                return queue.remove(pos);
            }
        }

        None
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_dequeue() {
        let queue = TaskQueue::new();
        let task = TileTask::new(TileCoord::new(0, 0, 0), Priority::Normal, true);
        
        queue.enqueue(task.clone());
        assert_eq!(queue.size(), 1);
        
        let dequeued = queue.dequeue();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().coord, task.coord);
        assert_eq!(queue.size(), 0);
    }

    #[test]
    fn test_priority_ordering() {
        let queue = TaskQueue::new();
        
        // Enqueue in random order
        let low_task = TileTask::new(TileCoord::new(0, 0, 0), Priority::Low, true);
        let high_task = TileTask::new(TileCoord::new(0, 1, 0), Priority::High, true);
        let normal_task = TileTask::new(TileCoord::new(0, 2, 0), Priority::Normal, true);
        
        queue.enqueue(low_task.clone());
        queue.enqueue(normal_task.clone());
        queue.enqueue(high_task.clone());
        
        // Dequeue should return in priority order: high, normal, low
        let first = queue.dequeue().unwrap();
        assert_eq!(first.priority, Priority::High);
        assert_eq!(first.coord, high_task.coord);
        
        let second = queue.dequeue().unwrap();
        assert_eq!(second.priority, Priority::Normal);
        assert_eq!(second.coord, normal_task.coord);
        
        let third = queue.dequeue().unwrap();
        assert_eq!(third.priority, Priority::Low);
        assert_eq!(third.coord, low_task.coord);
        
        assert!(queue.dequeue().is_none());
    }

    #[test]
    fn test_update_priority() {
        let queue = TaskQueue::new();
        let coord = TileCoord::new(0, 5, 5);
        let task = TileTask::new(coord, Priority::Low, true);
        
        queue.enqueue(task);
        assert_eq!(queue.size(), 1);
        
        // Update priority from Low to High
        queue.update_priority(coord, Priority::High);
        
        let dequeued = queue.dequeue().unwrap();
        assert_eq!(dequeued.priority, Priority::High);
        assert_eq!(dequeued.coord, coord);
    }

    #[test]
    fn test_remove() {
        let queue = TaskQueue::new();
        let coord1 = TileCoord::new(0, 0, 0);
        let coord2 = TileCoord::new(0, 1, 0);
        
        let task1 = TileTask::new(coord1, Priority::High, true);
        let task2 = TileTask::new(coord2, Priority::Normal, true);
        
        queue.enqueue(task1);
        queue.enqueue(task2);
        assert_eq!(queue.size(), 2);
        
        let removed = queue.remove(coord1);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().coord, coord1);
        assert_eq!(queue.size(), 1);
        
        let remaining = queue.dequeue().unwrap();
        assert_eq!(remaining.coord, coord2);
    }

    #[test]
    fn test_remove_nonexistent() {
        let queue = TaskQueue::new();
        let coord = TileCoord::new(0, 0, 0);
        
        let removed = queue.remove(coord);
        assert!(removed.is_none());
    }

    #[test]
    fn test_empty_queue() {
        let queue = TaskQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.size(), 0);
        assert!(queue.dequeue().is_none());
    }

    #[test]
    fn test_multiple_same_priority() {
        let queue = TaskQueue::new();
        
        let task1 = TileTask::new(TileCoord::new(0, 0, 0), Priority::High, true);
        let task2 = TileTask::new(TileCoord::new(0, 1, 0), Priority::High, true);
        let task3 = TileTask::new(TileCoord::new(0, 2, 0), Priority::High, true);
        
        queue.enqueue(task1.clone());
        queue.enqueue(task2.clone());
        queue.enqueue(task3.clone());
        
        // All queues use LIFO (most recent first)
        let first = queue.dequeue().unwrap();
        assert_eq!(first.coord, task3.coord);
        
        let second = queue.dequeue().unwrap();
        assert_eq!(second.coord, task2.coord);
        
        let third = queue.dequeue().unwrap();
        assert_eq!(third.coord, task1.coord);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let queue = Arc::new(TaskQueue::new());
        let mut handles = vec![];

        // Spawn multiple threads enqueueing tasks
        for i in 0..10 {
            let q = Arc::clone(&queue);
            let handle = thread::spawn(move || {
                for j in 0..10 {
                    let coord = TileCoord::new(0, i, j);
                    let task = TileTask::new(coord, Priority::Normal, true);
                    q.enqueue(task);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().unwrap();
        }

        // Should have 100 tasks total
        assert_eq!(queue.size(), 100);

        // Dequeue all tasks
        let mut count = 0;
        while queue.dequeue().is_some() {
            count += 1;
        }
        assert_eq!(count, 100);
        assert!(queue.is_empty());
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    /// Property 21: Task queue priority levels
    /// Verify three distinct priority levels and processing order
    ///
    /// **Validates: Requirements 9.1, 9.6**
    #[test]
    #[ignore]
    fn prop_priority_levels_and_ordering() {
        proptest!(|(
            high_count in 0usize..20,
            normal_count in 0usize..20,
            low_count in 0usize..20,
        )| {
            let queue = TaskQueue::new();

            // Enqueue tasks with different priorities
            for i in 0..high_count {
                let coord = TileCoord::new(0, i as u32, 0);
                let task = TileTask::new(coord, Priority::High, true);
                queue.enqueue(task);
            }

            for i in 0..normal_count {
                let coord = TileCoord::new(0, (high_count + i) as u32, 0);
                let task = TileTask::new(coord, Priority::Normal, true);
                queue.enqueue(task);
            }

            for i in 0..low_count {
                let coord = TileCoord::new(0, (high_count + normal_count + i) as u32, 0);
                let task = TileTask::new(coord, Priority::Low, true);
                queue.enqueue(task);
            }

            // Verify total count
            prop_assert_eq!(queue.size(), high_count + normal_count + low_count);

            // Dequeue and verify priority order
            let mut dequeued_high = 0;
            let mut dequeued_normal = 0;
            let mut dequeued_low = 0;
            let mut last_priority = Priority::High;

            while let Some(task) = queue.dequeue() {
                // Verify priority doesn't decrease
                prop_assert!(task.priority <= last_priority || 
                    (last_priority == Priority::High && task.priority == Priority::Normal) ||
                    (last_priority == Priority::Normal && task.priority == Priority::Low));

                match task.priority {
                    Priority::High => dequeued_high += 1,
                    Priority::Normal => dequeued_normal += 1,
                    Priority::Low => dequeued_low += 1,
                }

                last_priority = task.priority;
            }

            // Verify all tasks were dequeued
            prop_assert_eq!(dequeued_high, high_count);
            prop_assert_eq!(dequeued_normal, normal_count);
            prop_assert_eq!(dequeued_low, low_count);

            // Verify queue is empty
            prop_assert!(queue.is_empty());
        });
    }

    /// Property 24: Thread-safe queue access
    /// Verify concurrent access from multiple workers maintains integrity
    ///
    /// **Validates: Requirements 9.7**
    #[test]
    #[ignore]
    fn prop_thread_safe_concurrent_access() {
        proptest!(|(
            num_threads in 2usize..10,
            tasks_per_thread in 1usize..50,
        )| {
            use std::sync::Arc;
            use std::thread;

            let queue = Arc::new(TaskQueue::new());
            let mut handles = vec![];

            // Spawn multiple threads enqueueing tasks
            for thread_id in 0..num_threads {
                let q = Arc::clone(&queue);
                let handle = thread::spawn(move || {
                    for task_id in 0..tasks_per_thread {
                        let coord = TileCoord::new(0, thread_id as u32, task_id as u32);
                        let task = TileTask::new(coord, Priority::Normal, true);
                        q.enqueue(task);
                    }
                });
                handles.push(handle);
            }

            // Wait for all threads to complete
            for handle in handles {
                handle.join().unwrap();
            }

            // Verify total count
            let expected_total = num_threads * tasks_per_thread;
            prop_assert_eq!(queue.size(), expected_total);

            // Dequeue all tasks and verify count
            let mut dequeued_count = 0;
            while queue.dequeue().is_some() {
                dequeued_count += 1;
            }

            prop_assert_eq!(dequeued_count, expected_total);
            prop_assert!(queue.is_empty());
        });
    }

    /// Property: Enqueue and dequeue consistency
    /// Verify that all enqueued tasks can be dequeued
    #[test]
    #[ignore]
    fn prop_enqueue_dequeue_consistency() {
        proptest!(|(
            tasks in prop::collection::vec(
                (0u32..100, 0u32..100, any::<bool>()),
                1..100
            )
        )| {
            let queue = TaskQueue::new();

            // Enqueue all tasks
            for (x, y, is_high_res) in &tasks {
                let coord = TileCoord::new(0, *x, *y);
                let task = TileTask::new(coord, Priority::Normal, *is_high_res);
                queue.enqueue(task);
            }

            // Verify size
            prop_assert_eq!(queue.size(), tasks.len());

            // Dequeue all tasks
            let mut dequeued = vec![];
            while let Some(task) = queue.dequeue() {
                dequeued.push(task);
            }

            // Verify all tasks were dequeued
            prop_assert_eq!(dequeued.len(), tasks.len());
            prop_assert!(queue.is_empty());
        });
    }

    /// Property: Update priority maintains task
    /// Verify that updating priority doesn't lose the task
    #[test]
    #[ignore]
    fn prop_update_priority_maintains_task() {
        proptest!(|(
            coords in prop::collection::vec(
                (0u32..100, 0u32..100),
                1..50
            )
        )| {
            let queue = TaskQueue::new();

            // Enqueue tasks with Low priority (use unique coordinates)
            let unique_coords: Vec<_> = coords.iter().cloned().collect::<std::collections::HashSet<_>>().into_iter().collect();
            
            for (x, y) in &unique_coords {
                let coord = TileCoord::new(0, *x, *y);
                let task = TileTask::new(coord, Priority::Low, true);
                queue.enqueue(task);
            }

            let initial_size = queue.size();

            // Update all to High priority
            for (x, y) in &unique_coords {
                let coord = TileCoord::new(0, *x, *y);
                queue.update_priority(coord, Priority::High);
            }

            // Verify size unchanged
            prop_assert_eq!(queue.size(), initial_size);

            // Verify all tasks are now High priority
            let mut high_count = 0;
            while let Some(task) = queue.dequeue() {
                prop_assert_eq!(task.priority, Priority::High);
                high_count += 1;
            }

            prop_assert_eq!(high_count, unique_coords.len());
        });
    }

    /// Property: Remove maintains queue integrity
    /// Verify that removing tasks doesn't corrupt the queue
    #[test]
    #[ignore]
    fn prop_remove_maintains_integrity() {
        proptest!(|(
            all_coords in prop::collection::vec(
                (0u32..100, 0u32..100),
                1..50
            ),
            remove_indices in prop::collection::vec(0usize..1, 0..25)
        )| {
            let queue = TaskQueue::new();

            // Enqueue all tasks
            for (x, y) in &all_coords {
                let coord = TileCoord::new(0, *x, *y);
                let task = TileTask::new(coord, Priority::Normal, true);
                queue.enqueue(task);
            }

            let initial_size = queue.size();

            // Remove some tasks
            let mut removed_count = 0;
            for &idx in &remove_indices {
                if idx < all_coords.len() {
                    let (x, y) = all_coords[idx];
                    let coord = TileCoord::new(0, x, y);
                    if queue.remove(coord).is_some() {
                        removed_count += 1;
                    }
                }
            }

            // Verify size decreased correctly
            prop_assert_eq!(queue.size(), initial_size - removed_count);

            // Verify remaining tasks can be dequeued
            let mut dequeued_count = 0;
            while queue.dequeue().is_some() {
                dequeued_count += 1;
            }

            prop_assert_eq!(dequeued_count, initial_size - removed_count);
            prop_assert!(queue.is_empty());
        });
    }
}
