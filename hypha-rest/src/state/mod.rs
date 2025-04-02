use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::{
    api::v0::{Task, TaskSpec},
    state::{error::StateError, persistence::StatePersistence},
};

pub mod error;
pub mod persistence;

type Result<T> = std::result::Result<T, StateError>;
type Responder<T> = oneshot::Sender<Result<T>>;

#[derive(Clone)]
pub struct State {
    sender: mpsc::Sender<Command>,
}

impl State {
    pub fn new<S>(persistence: S) -> Self
    where
        S: StatePersistence + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(32);
        tokio::spawn(command_handler(receiver, persistence));
        Self { sender }
    }

    pub async fn create_task(&self, spec: TaskSpec) -> Result<Task> {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(Command::CreateTask { spec, sender }).await;
        receiver.await.unwrap()
    }

    pub async fn read_task(&self, id: &Uuid) -> Result<Task> {
        let (sender, receiver) = oneshot::channel();
        let _ = self
            .sender
            .send(Command::ReadTask { id: *id, sender })
            .await;
        receiver.await.unwrap()
    }

    pub async fn update_task(&self, id: &Uuid, task: Task) -> Result<Task> {
        let (sender, receiver) = oneshot::channel();
        let _ = self
            .sender
            .send(Command::UpdateTask {
                id: *id,
                task,
                sender,
            })
            .await;
        receiver.await.unwrap()
    }

    pub async fn delete_task(&self, id: &Uuid) -> Result<Task> {
        let (sender, receiver) = oneshot::channel();
        let _ = self
            .sender
            .send(Command::DeleteTask { id: *id, sender })
            .await;
        receiver.await.unwrap()
    }

    pub async fn list_tasks(&self) -> Result<Vec<Task>> {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(Command::ListTasks { sender }).await;
        receiver.await.unwrap()
    }

    pub async fn watch_tasks(&self) -> Result<BroadcastStream<Task>> {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.send(Command::WatchTasks { sender }).await;
        receiver.await.unwrap()
    }
}

#[derive(Debug)]
enum Command {
    CreateTask {
        spec: TaskSpec,
        sender: Responder<Task>,
    },
    ReadTask {
        id: Uuid,
        sender: Responder<Task>,
    },
    UpdateTask {
        id: Uuid,
        task: Task,
        sender: Responder<Task>,
    },
    DeleteTask {
        id: Uuid,
        sender: Responder<Task>,
    },
    ListTasks {
        sender: Responder<Vec<Task>>,
    },
    WatchTasks {
        sender: Responder<BroadcastStream<Task>>,
    },
}

async fn command_handler<S>(mut receiver: mpsc::Receiver<Command>, mut persistence: S)
where
    S: StatePersistence + Send,
{
    let (publisher, _) = broadcast::channel(16);

    while let Some(command) = receiver.recv().await {
        match command {
            Command::CreateTask { spec, sender } => {
                let task = Task::new(spec);

                match persistence.put(&format!("task:{}", task.id), task.clone()) {
                    Ok(_) => {
                        let _ = sender.send(Ok(task.clone()));
                        let _ = publisher.send(task);
                    }
                    Err(e) => {
                        let _ = sender.send(Err(StateError::Internal(e)));
                    }
                }
            }
            Command::ReadTask { id, sender } => {
                match persistence.get::<Task>(&format!("task:{}", id)) {
                    Ok(task) => {
                        if let Some(task) = task {
                            let _ = sender.send(Ok(task.clone()));
                        } else {
                            let _ = sender.send(Err(StateError::NotFound(id)));
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Err(StateError::Internal(e)));
                    }
                }
            }
            Command::UpdateTask { id, task, sender } => {
                match persistence.get::<Task>(&format!("task:{}", id)) {
                    Ok(current_task) => {
                        if let Some(current_task) = current_task {
                            let mut current_task = current_task.clone();
                            current_task.update(&task);

                            match persistence.put(&format!("task:{}", id), current_task.clone()) {
                                Ok(_) => {
                                    let _ = sender.send(Ok(current_task.clone()));
                                    let _ = publisher.send(current_task);
                                }
                                Err(e) => {
                                    let _ = sender.send(Err(StateError::Internal(e)));
                                }
                            }
                        } else {
                            let _ = sender.send(Err(StateError::NotFound(id)));
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Err(StateError::Internal(e)));
                    }
                }
            }
            Command::DeleteTask { id, sender } => {
                match persistence.delete::<Task>(&format!("task:{}", id)) {
                    Ok(task) => {
                        if let Some(task) = task {
                            let _ = sender.send(Ok(task.clone()));
                            let mut deleted_task = task;
                            deleted_task.status.status = "deleted".to_string();
                            let _ = publisher.send(deleted_task);
                        } else {
                            let _ = sender.send(Err(StateError::NotFound(id)));
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(Err(StateError::Internal(e)));
                    }
                }
            }
            Command::ListTasks { sender } => match persistence.list("task:") {
                Ok(tasks) => {
                    let _ = sender.send(Ok(tasks));
                }
                Err(e) => {
                    let _ = sender.send(Err(StateError::Internal(e)));
                }
            },
            Command::WatchTasks { sender } => {
                let subscriber = publisher.subscribe();
                let stream = BroadcastStream::new(subscriber);
                let _ = sender.send(Ok(stream));
            }
        }
    }
}
