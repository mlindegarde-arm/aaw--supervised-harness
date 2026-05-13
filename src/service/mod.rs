use crate::HarnessResult;
use crate::domain::{Task, TaskId, Ticket, TicketId};
use crate::runtime::{CommandResult, ResumeTaskOptions, TaskRunOptions, TicketResolveOptions};

pub trait HarnessService {
    fn create_task(
        &self,
        title: String,
        goal: String,
        validation_commands: Vec<String>,
    ) -> HarnessResult<Task>;

    fn list_tasks(&self) -> HarnessResult<Vec<Task>>;
    fn get_task(&self, task_id: &TaskId) -> HarnessResult<Task>;
    fn run_task(&self, task_id: &TaskId, options: TaskRunOptions) -> HarnessResult<CommandResult>;
    fn list_tickets(&self) -> HarnessResult<Vec<Ticket>>;
    fn get_ticket(&self, ticket_id: &TicketId) -> HarnessResult<Ticket>;
    fn resolve_ticket(
        &self,
        ticket_id: &TicketId,
        options: TicketResolveOptions,
    ) -> HarnessResult<CommandResult>;
    fn resume_task(
        &self,
        task_id: &TaskId,
        options: ResumeTaskOptions,
    ) -> HarnessResult<CommandResult>;
}
