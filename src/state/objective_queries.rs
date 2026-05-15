use super::{
    NewGeneratedTask, NewObjectiveMessage, NewObjectiveResolverAttempt, Objective,
    ObjectiveAcceptanceCriterion, ObjectiveArtifact, ObjectiveEvent, ObjectiveMessage,
    ObjectiveMonitorLease, ObjectivePlanBundle, ObjectivePlanBundleResult,
    ObjectiveResolverAttempt, ObjectiveStatusUpdate, ObjectiveStore, ObjectiveTask,
    ObjectiveTaskStatusCounts, ObjectiveValidationCommand, PlannerExchange,
    RejectedPlannerExchange, SqliteTaskStore, collect_rows, ensure_exists, insert_task_row,
    not_found, now_string, parse_id, parse_optional_id, parse_status, sql_err,
};
use crate::domain::{
    ObjectiveAcceptanceStatus, ObjectiveArtifactId, ObjectiveId, ObjectivePlanId,
    ObjectiveResolverAttemptId, ObjectiveStatus, ObjectiveValidationCommandSource,
    ObjectiveValidationReviewStatus, PlannerExchangeId, PlannerExchangeKind, TaskId, TaskStatus,
    TicketId,
};
use crate::{HarnessError, HarnessResult};
use rusqlite::{Connection, OptionalExtension, params};

impl ObjectiveStore for SqliteTaskStore {
    fn insert_objective(&self, objective: Objective) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO objectives
             (id, title, prompt, summary, status, planner_model, worker_model, ticket_model,
              active_plan_id, monitor_lease_owner, monitor_lease_expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                objective.id.as_str(),
                objective.title,
                objective.prompt,
                objective.summary,
                objective.status.as_str(),
                objective.planner_model,
                objective.worker_model,
                objective.ticket_model,
                objective
                    .active_plan_id
                    .as_ref()
                    .map(ObjectivePlanId::as_str),
                objective.monitor_lease_owner,
                objective.monitor_lease_expires_at,
                objective.created_at,
                objective.updated_at,
            ],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn get_objective(&self, objective_id: &ObjectiveId) -> HarnessResult<Objective> {
        let conn = self.lock_conn()?;
        get_objective_with_conn(&conn, objective_id)
    }

    fn list_objectives(&self, status: Option<ObjectiveStatus>) -> HarnessResult<Vec<Objective>> {
        let conn = self.lock_conn()?;
        let sql = match status {
            Some(_) => "SELECT * FROM objectives WHERE status = ?1 ORDER BY created_at ASC, id ASC",
            None => "SELECT * FROM objectives ORDER BY created_at ASC, id ASC",
        };
        let mut stmt = conn.prepare(sql).map_err(sql_err)?;
        let rows = match status {
            Some(status) => stmt
                .query_map(params![status.as_str()], row_to_objective)
                .map_err(sql_err)?,
            None => stmt.query_map([], row_to_objective).map_err(sql_err)?,
        };
        collect_rows(rows)
    }

    fn update_objective_status(
        &self,
        objective_id: &ObjectiveId,
        expected: Option<ObjectiveStatus>,
        update: ObjectiveStatusUpdate,
    ) -> HarnessResult<Objective> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let mut objective = get_objective_with_conn(&tx, objective_id)?;
        if let Some(expected) = expected
            && objective.status != expected
        {
            return Err(HarnessError::Conflict(format!(
                "objective {} was not in status {expected}",
                objective.id
            )));
        }

        if let Some(status) = update.status {
            objective.status = status;
        }
        if let Some(summary) = update.summary {
            objective.summary = summary;
        }
        if let Some(active_plan_id) = update.active_plan_id {
            objective.active_plan_id = active_plan_id;
        }
        if let Some(updated_at) = update.updated_at {
            objective.updated_at = updated_at;
        }

        let rows = tx
            .execute(
                "UPDATE objectives
                 SET title = ?2, prompt = ?3, summary = ?4, status = ?5,
                     planner_model = ?6, worker_model = ?7, ticket_model = ?8,
                     active_plan_id = ?9, monitor_lease_owner = ?10,
                     monitor_lease_expires_at = ?11, created_at = ?12, updated_at = ?13
                 WHERE id = ?1",
                params![
                    objective.id.as_str(),
                    objective.title,
                    objective.prompt,
                    objective.summary,
                    objective.status.as_str(),
                    objective.planner_model,
                    objective.worker_model,
                    objective.ticket_model,
                    objective
                        .active_plan_id
                        .as_ref()
                        .map(ObjectivePlanId::as_str),
                    objective.monitor_lease_owner,
                    objective.monitor_lease_expires_at,
                    objective.created_at,
                    objective.updated_at,
                ],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(&tx, "objectives", objective_id.as_str(), "objective")?;
        }
        let objective = get_objective_with_conn(&tx, objective_id)?;
        tx.commit().map_err(sql_err)?;
        Ok(objective)
    }

    fn insert_objective_artifact(&self, artifact: ObjectiveArtifact) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        insert_objective_artifact_row(&conn, &artifact)
    }

    fn list_objective_artifacts(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<ObjectiveArtifact>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM objective_artifacts
                 WHERE objective_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![objective_id.as_str()], row_to_objective_artifact)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn insert_objective_message(
        &self,
        message: NewObjectiveMessage,
    ) -> HarnessResult<ObjectiveMessage> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        assert_message_matches_objective(&tx, &message, &message.objective_id)?;
        let message = insert_objective_message_row(&tx, &message)?;
        tx.commit().map_err(sql_err)?;
        Ok(message)
    }

    fn list_objective_messages(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<ObjectiveMessage>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM objective_messages
                 WHERE objective_id = ?1
                 ORDER BY sequence ASC, id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![objective_id.as_str()], row_to_objective_message)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn insert_objective_event(&self, event: ObjectiveEvent) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        insert_objective_event_row(&conn, &event)
    }

    fn list_objective_events(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<ObjectiveEvent>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM objective_events
                 WHERE objective_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![objective_id.as_str()], row_to_objective_event)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn insert_planner_exchange(&self, exchange: PlannerExchange) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        insert_planner_exchange_row(&conn, &exchange)
    }

    fn list_planner_exchanges(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<PlannerExchange>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM planner_exchanges
                 WHERE objective_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![objective_id.as_str()], row_to_planner_exchange)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn reject_objective_plan(
        &self,
        objective_id: &ObjectiveId,
        exchange: RejectedPlannerExchange,
        event: ObjectiveEvent,
    ) -> HarnessResult<()> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        ensure_exists(&tx, "objectives", objective_id.as_str(), "objective")?;
        assert_exchange_matches_objective(&exchange.exchange, objective_id)?;
        if exchange.exchange.status != "rejected" {
            return Err(HarnessError::Conflict(
                "rejected planner exchange must have status rejected".to_string(),
            ));
        }
        for artifact in &exchange.artifacts {
            assert_artifact_matches_objective(artifact, objective_id)?;
            insert_objective_artifact_row(&tx, artifact)?;
        }
        insert_planner_exchange_row(&tx, &exchange.exchange)?;
        let mut messages = Vec::new();
        for message in &exchange.messages {
            assert_message_matches_objective(&tx, message, objective_id)?;
            messages.push(insert_objective_message_row(&tx, message)?);
        }
        assert_event_matches_objective(&event, objective_id)?;
        insert_objective_event_row(&tx, &event)?;
        tx.execute(
            "UPDATE objectives
             SET status = ?2, updated_at = ?3
             WHERE id = ?1",
            params![
                objective_id.as_str(),
                ObjectiveStatus::Failed.as_str(),
                event.created_at
            ],
        )
        .map_err(sql_err)?;
        drop(messages);
        tx.commit().map_err(sql_err)
    }

    fn create_objective_plan_bundle(
        &self,
        objective_id: &ObjectiveId,
        bundle: ObjectivePlanBundle,
    ) -> HarnessResult<ObjectivePlanBundleResult> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        ensure_exists(&tx, "objectives", objective_id.as_str(), "objective")?;
        if &bundle.plan.objective_id != objective_id {
            return Err(HarnessError::Conflict(format!(
                "plan {} does not belong to objective {}",
                bundle.plan.id, objective_id
            )));
        }
        assert_exchange_matches_objective(&bundle.exchange, objective_id)?;
        assert_initial_plan_exchange(&bundle.exchange)?;

        tx.execute(
            "INSERT INTO objective_plans (id, objective_id, version, summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                bundle.plan.id.as_str(),
                bundle.plan.objective_id.as_str(),
                bundle.plan.version as i64,
                bundle.plan.summary,
                bundle.plan.created_at,
            ],
        )
        .map_err(sql_err)?;

        for criterion in &bundle.acceptance_criteria {
            if &criterion.objective_id != objective_id || criterion.plan_id != bundle.plan.id {
                return Err(HarnessError::Conflict(
                    "acceptance criterion does not match objective plan".to_string(),
                ));
            }
            tx.execute(
                "INSERT INTO objective_acceptance_criteria
                 (id, objective_id, plan_id, description, status, last_evaluated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    criterion.id.as_str(),
                    criterion.objective_id.as_str(),
                    criterion.plan_id.as_str(),
                    criterion.description,
                    criterion.status.as_str(),
                    criterion.last_evaluated_at,
                ],
            )
            .map_err(sql_err)?;
        }

        for command in &bundle.validation_commands {
            if &command.objective_id != objective_id || command.plan_id != bundle.plan.id {
                return Err(HarnessError::Conflict(
                    "objective validation command does not match objective plan".to_string(),
                ));
            }
            tx.execute(
                "INSERT INTO objective_validation_commands
                 (id, objective_id, plan_id, command, source, review_status, review_reason, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    command.id.as_str(),
                    command.objective_id.as_str(),
                    command.plan_id.as_str(),
                    command.command,
                    command.source.as_str(),
                    command.review_status.as_str(),
                    command.review_reason,
                    command.created_at,
                ],
            )
            .map_err(sql_err)?;
        }

        for artifact in &bundle.artifacts {
            assert_artifact_matches_objective(artifact, objective_id)?;
            insert_objective_artifact_row(&tx, artifact)?;
        }
        insert_planner_exchange_row(&tx, &bundle.exchange)?;

        let mut objective_tasks = Vec::new();
        for generated in &bundle.generated_tasks {
            insert_task_row(&tx, &generated.task)?;
            for (idx, command) in generated.trusted_validation_commands.iter().enumerate() {
                tx.execute(
                    "INSERT INTO task_validation_commands (task_id, position, command, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        generated.task.id.as_str(),
                        idx as i64,
                        command,
                        generated.task.created_at,
                    ],
                )
                .map_err(sql_err)?;
            }
            let objective_task = ObjectiveTask {
                objective_id: objective_id.clone(),
                task_id: generated.task.id.clone(),
                plan_id: bundle.plan.id.clone(),
                task_key: generated.task_key.clone(),
                parallel_group: generated.parallel_group.clone(),
                owned_paths_json: generated.owned_paths_json.clone(),
                sequence: generated.sequence,
                worker_attempt_budget: generated.worker_attempt_budget,
                worker_attempts_used: 0,
            };
            insert_objective_task_row(&tx, &objective_task)?;
            for command in &generated.reviewed_validation_commands {
                if command.objective_id != *objective_id || command.task_id != generated.task.id {
                    return Err(HarnessError::Conflict(
                        "task validation command does not match generated task".to_string(),
                    ));
                }
                insert_objective_task_validation_command_row(&tx, command)?;
            }
            objective_tasks.push(objective_task);
        }

        for dependency in &bundle.dependencies {
            assert_objective_task_link(&tx, objective_id, &bundle.plan.id, &dependency.task_id)?;
            assert_objective_task_link(
                &tx,
                objective_id,
                &bundle.plan.id,
                &dependency.depends_on_task_id,
            )?;
            tx.execute(
                "INSERT INTO objective_task_dependencies
                 (objective_id, task_id, depends_on_task_id)
                 VALUES (?1, ?2, ?3)",
                params![
                    objective_id.as_str(),
                    dependency.task_id.as_str(),
                    dependency.depends_on_task_id.as_str(),
                ],
            )
            .map_err(sql_err)?;
        }

        let mut messages = Vec::new();
        for message in &bundle.messages {
            assert_message_matches_objective(&tx, message, objective_id)?;
            messages.push(insert_objective_message_row(&tx, message)?);
        }
        for event in &bundle.events {
            assert_event_matches_objective(event, objective_id)?;
            insert_objective_event_row(&tx, event)?;
        }
        tx.execute(
            "UPDATE objectives
             SET status = ?2, active_plan_id = ?3, summary = ?4, updated_at = ?5
             WHERE id = ?1",
            params![
                objective_id.as_str(),
                ObjectiveStatus::Ready.as_str(),
                bundle.plan.id.as_str(),
                bundle.plan.summary,
                bundle.objective_updated_at,
            ],
        )
        .map_err(sql_err)?;
        let objective = get_objective_with_conn(&tx, objective_id)?;
        tx.commit().map_err(sql_err)?;
        Ok(ObjectivePlanBundleResult {
            objective,
            plan: bundle.plan,
            tasks: objective_tasks,
            messages,
        })
    }

    fn next_ready_objective_task(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Option<crate::domain::Task>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT t.*
             FROM objective_tasks ot
             JOIN objectives objective ON objective.id = ot.objective_id
             JOIN tasks t ON t.id = ot.task_id
             WHERE ot.objective_id = ?1
               AND objective.active_plan_id = ot.plan_id
               AND t.status = ?2
               AND NOT EXISTS (
                   SELECT 1
                   FROM objective_task_dependencies dep
                   JOIN tasks dep_task ON dep_task.id = dep.depends_on_task_id
                   WHERE dep.objective_id = ot.objective_id
                     AND dep.task_id = ot.task_id
                     AND dep_task.status != ?3
               )
             ORDER BY ot.sequence ASC, ot.task_id ASC
             LIMIT 1",
            params![
                objective_id.as_str(),
                TaskStatus::Ready.as_str(),
                TaskStatus::Complete.as_str(),
            ],
            super::row_to_task,
        )
        .optional()
        .map_err(sql_err)
    }

    fn next_stuck_objective_task(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Option<crate::domain::Task>> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT t.*
             FROM objective_tasks ot
             JOIN objectives objective ON objective.id = ot.objective_id
             JOIN tasks t ON t.id = ot.task_id
             WHERE ot.objective_id = ?1
               AND objective.active_plan_id = ot.plan_id
               AND t.status = ?2
             ORDER BY ot.sequence ASC, ot.task_id ASC
             LIMIT 1",
            params![objective_id.as_str(), TaskStatus::Stuck.as_str()],
            super::row_to_task,
        )
        .optional()
        .map_err(sql_err)
    }

    fn get_objective_task(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
    ) -> HarnessResult<ObjectiveTask> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT * FROM objective_tasks WHERE objective_id = ?1 AND task_id = ?2",
            params![objective_id.as_str(), task_id.as_str()],
            row_to_objective_task,
        )
        .optional()
        .map_err(sql_err)?
        .ok_or_else(|| HarnessError::NotFound {
            kind: "objective_task",
            id: format!("{}:{}", objective_id.as_str(), task_id.as_str()),
        })
    }

    fn list_active_objective_task_ids(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<TaskId>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT ot.task_id
                 FROM objective_tasks ot
                 JOIN objectives objective ON objective.id = ot.objective_id
                 WHERE ot.objective_id = ?1
                   AND objective.active_plan_id = ot.plan_id
                 ORDER BY ot.sequence ASC, ot.task_id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![objective_id.as_str()], |row| {
                super::parse_id(row.get::<_, String>(0)?, 0)
            })
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn increment_objective_task_attempts_used(
        &self,
        objective_id: &ObjectiveId,
        task_id: &TaskId,
        delta: u32,
    ) -> HarnessResult<ObjectiveTask> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE objective_tasks
                 SET worker_attempts_used = worker_attempts_used + ?3
                 WHERE objective_id = ?1 AND task_id = ?2",
                params![objective_id.as_str(), task_id.as_str(), delta as i64],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            return Err(HarnessError::Conflict(format!(
                "task {} is not linked to objective {}",
                task_id, objective_id
            )));
        }
        conn.query_row(
            "SELECT * FROM objective_tasks WHERE objective_id = ?1 AND task_id = ?2",
            params![objective_id.as_str(), task_id.as_str()],
            row_to_objective_task,
        )
        .map_err(sql_err)
    }

    fn active_objective_task_status_counts(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<ObjectiveTaskStatusCounts> {
        let conn = self.lock_conn()?;
        conn.query_row(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CASE WHEN t.status = 'ready' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status = 'running' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status = 'stuck' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status = 'complete' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.status = 'failed' THEN 1 ELSE 0 END), 0)
             FROM objective_tasks ot
             JOIN objectives objective ON objective.id = ot.objective_id
             JOIN tasks t ON t.id = ot.task_id
             WHERE ot.objective_id = ?1
               AND objective.active_plan_id = ot.plan_id",
            params![objective_id.as_str()],
            |row| {
                Ok(ObjectiveTaskStatusCounts {
                    total: row.get::<_, i64>(0)? as u32,
                    ready: row.get::<_, i64>(1)? as u32,
                    running: row.get::<_, i64>(2)? as u32,
                    stuck: row.get::<_, i64>(3)? as u32,
                    complete: row.get::<_, i64>(4)? as u32,
                    failed: row.get::<_, i64>(5)? as u32,
                })
            },
        )
        .map_err(sql_err)
    }

    fn list_active_objective_acceptance_criteria(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<ObjectiveAcceptanceCriterion>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT criteria.*
                 FROM objective_acceptance_criteria criteria
                 JOIN objectives objective
                   ON objective.id = criteria.objective_id
                  AND objective.active_plan_id = criteria.plan_id
                 WHERE criteria.objective_id = ?1
                 ORDER BY criteria.id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![objective_id.as_str()], row_to_acceptance_criterion)
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn list_active_objective_validation_commands(
        &self,
        objective_id: &ObjectiveId,
    ) -> HarnessResult<Vec<ObjectiveValidationCommand>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT validation.*
                 FROM objective_validation_commands validation
                 JOIN objectives objective
                   ON objective.id = validation.objective_id
                  AND objective.active_plan_id = validation.plan_id
                 WHERE validation.objective_id = ?1
                 ORDER BY validation.id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(
                params![objective_id.as_str()],
                row_to_objective_validation_command,
            )
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn update_active_objective_acceptance_status(
        &self,
        objective_id: &ObjectiveId,
        status: ObjectiveAcceptanceStatus,
        evaluated_at: &str,
    ) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        ensure_exists(&conn, "objectives", objective_id.as_str(), "objective")?;
        conn.execute(
            "UPDATE objective_acceptance_criteria
             SET status = ?2, last_evaluated_at = ?3
             WHERE objective_id = ?1
               AND plan_id = (
                   SELECT active_plan_id FROM objectives WHERE id = ?1
               )",
            params![objective_id.as_str(), status.as_str(), evaluated_at],
        )
        .map_err(sql_err)?;
        Ok(())
    }

    fn create_objective_repair_task(
        &self,
        objective_id: &ObjectiveId,
        generated: NewGeneratedTask,
    ) -> HarnessResult<ObjectiveTask> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let objective = get_objective_with_conn(&tx, objective_id)?;
        let plan_id = objective.active_plan_id.ok_or_else(|| {
            HarnessError::Conflict(format!("objective {} has no active plan", objective_id))
        })?;
        insert_task_row(&tx, &generated.task)?;
        for (idx, command) in generated.trusted_validation_commands.iter().enumerate() {
            tx.execute(
                "INSERT INTO task_validation_commands (task_id, position, command, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    generated.task.id.as_str(),
                    idx as i64,
                    command,
                    generated.task.created_at,
                ],
            )
            .map_err(sql_err)?;
        }
        let objective_task = ObjectiveTask {
            objective_id: objective_id.clone(),
            task_id: generated.task.id.clone(),
            plan_id,
            task_key: generated.task_key,
            parallel_group: generated.parallel_group,
            owned_paths_json: generated.owned_paths_json,
            sequence: generated.sequence,
            worker_attempt_budget: generated.worker_attempt_budget,
            worker_attempts_used: 0,
        };
        insert_objective_task_row(&tx, &objective_task)?;
        tx.commit().map_err(sql_err)?;
        Ok(objective_task)
    }

    fn create_resolver_attempt(
        &self,
        attempt: NewObjectiveResolverAttempt,
    ) -> HarnessResult<ObjectiveResolverAttempt> {
        let conn = self.lock_conn()?;
        assert_ticket_belongs_to_objective(&conn, &attempt.ticket_id, &attempt.objective_id)?;
        conn.execute(
            "INSERT INTO objective_ticket_resolver_attempts
             (id, objective_id, ticket_id, attempt, status, lease_owner, lease_expires_at,
              planner_exchange_id, last_error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'queued', NULL, NULL, NULL, NULL, ?5, ?5)",
            params![
                attempt.id.as_str(),
                attempt.objective_id.as_str(),
                attempt.ticket_id.as_str(),
                attempt.attempt as i64,
                attempt.created_at,
            ],
        )
        .map_err(sql_err)?;
        get_resolver_attempt_with_conn(&conn, &attempt.id)
    }

    fn list_resolver_attempts_for_ticket(
        &self,
        objective_id: &ObjectiveId,
        ticket_id: &TicketId,
    ) -> HarnessResult<Vec<ObjectiveResolverAttempt>> {
        let conn = self.lock_conn()?;
        assert_ticket_belongs_to_objective(&conn, ticket_id, objective_id)?;
        let mut stmt = conn
            .prepare(
                "SELECT *
                 FROM objective_ticket_resolver_attempts
                 WHERE objective_id = ?1 AND ticket_id = ?2
                 ORDER BY attempt ASC, created_at ASC, id ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(
                params![objective_id.as_str(), ticket_id.as_str()],
                row_to_resolver_attempt,
            )
            .map_err(sql_err)?;
        collect_rows(rows)
    }

    fn acquire_resolver_attempt_lease(
        &self,
        attempt_id: &ObjectiveResolverAttemptId,
        owner: &str,
    ) -> HarnessResult<ObjectiveResolverAttempt> {
        let now = super::current_unix_secs();
        let expires_at = (now + self.lease_ttl_secs).to_string();
        let updated_at = now.to_string();
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction().map_err(sql_err)?;
        let attempt = get_resolver_attempt_with_conn(&tx, attempt_id)?;
        let active_for_ticket: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM objective_ticket_resolver_attempts
                    WHERE ticket_id = ?1
                      AND id != ?2
                      AND status = 'resolving'
                      AND lease_owner IS NOT NULL
                      AND lease_expires_at IS NOT NULL
                      AND CAST(lease_expires_at AS INTEGER) > ?3
                 )",
                params![attempt.ticket_id.as_str(), attempt_id.as_str(), now],
                |row| row.get(0),
            )
            .map_err(sql_err)?;
        if active_for_ticket {
            return Err(HarnessError::Conflict(format!(
                "ticket {} already has an active resolver lease",
                attempt.ticket_id
            )));
        }
        let rows = tx
            .execute(
                "UPDATE objective_ticket_resolver_attempts
                 SET status = 'resolving', lease_owner = ?2, lease_expires_at = ?3, updated_at = ?4
                 WHERE id = ?1
                   AND status IN ('queued','failed','resolving')
                   AND (lease_owner IS NULL OR lease_expires_at IS NULL OR CAST(lease_expires_at AS INTEGER) <= ?5)",
                params![attempt_id.as_str(), owner, expires_at, updated_at, now],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            return Err(HarnessError::Conflict(format!(
                "resolver attempt {} already has a non-expired lease",
                attempt_id
            )));
        }
        let attempt = get_resolver_attempt_with_conn(&tx, attempt_id)?;
        tx.commit().map_err(sql_err)?;
        Ok(attempt)
    }

    fn release_resolver_attempt_lease(
        &self,
        attempt_id: &ObjectiveResolverAttemptId,
        owner: &str,
        status: &str,
        planner_exchange_id: Option<&PlannerExchangeId>,
        last_error: Option<&str>,
    ) -> HarnessResult<ObjectiveResolverAttempt> {
        if !matches!(status, "queued" | "resolved" | "failed") {
            return Err(HarnessError::Conflict(format!(
                "invalid resolver release status {status}"
            )));
        }
        let conn = self.lock_conn()?;
        if let Some(exchange_id) = planner_exchange_id {
            let attempt = get_resolver_attempt_with_conn(&conn, attempt_id)?;
            assert_ticket_resolution_exchange_matches_attempt(&conn, exchange_id, &attempt)?;
        }
        let now = super::current_unix_secs();
        let rows = conn
            .execute(
                "UPDATE objective_ticket_resolver_attempts
                 SET status = ?3, lease_owner = NULL, lease_expires_at = NULL,
                     planner_exchange_id = COALESCE(?4, planner_exchange_id),
                     last_error = ?5, updated_at = ?6
                 WHERE id = ?1
                   AND lease_owner = ?2
                   AND lease_expires_at IS NOT NULL
                   AND CAST(lease_expires_at AS INTEGER) > ?7",
                params![
                    attempt_id.as_str(),
                    owner,
                    status,
                    planner_exchange_id.map(PlannerExchangeId::as_str),
                    last_error,
                    now_string(),
                    now,
                ],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(
                &conn,
                "objective_ticket_resolver_attempts",
                attempt_id.as_str(),
                "objective_resolver_attempt",
            )?;
            return Err(HarnessError::Conflict(format!(
                "resolver attempt {} lease is not held by {owner}",
                attempt_id
            )));
        }
        get_resolver_attempt_with_conn(&conn, attempt_id)
    }

    fn acquire_objective_monitor_lease(
        &self,
        objective_id: &ObjectiveId,
        owner: &str,
    ) -> HarnessResult<ObjectiveMonitorLease> {
        update_monitor_lease(self, objective_id, owner, false)
    }

    fn refresh_objective_monitor_lease(
        &self,
        objective_id: &ObjectiveId,
        owner: &str,
    ) -> HarnessResult<ObjectiveMonitorLease> {
        update_monitor_lease(self, objective_id, owner, true)
    }

    fn release_objective_monitor_lease(
        &self,
        objective_id: &ObjectiveId,
        owner: &str,
    ) -> HarnessResult<()> {
        let conn = self.lock_conn()?;
        let rows = conn
            .execute(
                "UPDATE objectives
                 SET monitor_lease_owner = NULL, monitor_lease_expires_at = NULL, updated_at = ?3
                 WHERE id = ?1 AND monitor_lease_owner = ?2",
                params![objective_id.as_str(), owner, now_string()],
            )
            .map_err(sql_err)?;
        if rows == 0 {
            ensure_exists(&conn, "objectives", objective_id.as_str(), "objective")?;
            return Err(HarnessError::Conflict(format!(
                "objective {} monitor lease is not held by {owner}",
                objective_id
            )));
        }
        Ok(())
    }
}

fn get_objective_with_conn(
    conn: &rusqlite::Connection,
    objective_id: &ObjectiveId,
) -> HarnessResult<Objective> {
    conn.query_row(
        "SELECT * FROM objectives WHERE id = ?1",
        params![objective_id.as_str()],
        row_to_objective,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("objective", objective_id.as_str()))
}

fn row_to_objective(row: &rusqlite::Row<'_>) -> rusqlite::Result<Objective> {
    Ok(Objective {
        id: super::parse_id(row.get(0)?, 0)?,
        title: row.get(1)?,
        prompt: row.get(2)?,
        summary: row.get(3)?,
        status: parse_status(row.get::<_, String>(4)?, 4)?,
        planner_model: row.get(5)?,
        worker_model: row.get(6)?,
        ticket_model: row.get(7)?,
        active_plan_id: parse_optional_id(row.get(8)?, 8)?,
        monitor_lease_owner: row.get(9)?,
        monitor_lease_expires_at: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn row_to_acceptance_criterion(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ObjectiveAcceptanceCriterion> {
    Ok(ObjectiveAcceptanceCriterion {
        id: super::parse_id(row.get(0)?, 0)?,
        objective_id: super::parse_id(row.get(1)?, 1)?,
        plan_id: super::parse_id(row.get(2)?, 2)?,
        description: row.get(3)?,
        status: parse_status::<ObjectiveAcceptanceStatus>(row.get::<_, String>(4)?, 4)?,
        last_evaluated_at: row.get(5)?,
    })
}

fn row_to_objective_validation_command(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ObjectiveValidationCommand> {
    Ok(ObjectiveValidationCommand {
        id: super::parse_id(row.get(0)?, 0)?,
        objective_id: super::parse_id(row.get(1)?, 1)?,
        plan_id: super::parse_id(row.get(2)?, 2)?,
        command: row.get(3)?,
        source: parse_status::<ObjectiveValidationCommandSource>(row.get::<_, String>(4)?, 4)?,
        review_status: parse_status::<ObjectiveValidationReviewStatus>(
            row.get::<_, String>(5)?,
            5,
        )?,
        review_reason: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn row_to_objective_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectiveTask> {
    Ok(ObjectiveTask {
        objective_id: super::parse_id(row.get(0)?, 0)?,
        task_id: super::parse_id(row.get(1)?, 1)?,
        plan_id: super::parse_id(row.get(2)?, 2)?,
        task_key: row.get(3)?,
        parallel_group: row.get(4)?,
        owned_paths_json: row.get(5)?,
        sequence: row.get::<_, i64>(6)? as u32,
        worker_attempt_budget: row.get::<_, i64>(7)? as u32,
        worker_attempts_used: row.get::<_, i64>(8)? as u32,
    })
}

fn insert_objective_artifact_row(
    conn: &Connection,
    artifact: &ObjectiveArtifact,
) -> HarnessResult<()> {
    ensure_exists(
        conn,
        "objectives",
        artifact.objective_id.as_str(),
        "objective",
    )?;
    if let Some(plan_id) = &artifact.plan_id {
        let plan_objective_id: String = conn
            .query_row(
                "SELECT objective_id FROM objective_plans WHERE id = ?1",
                params![plan_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_err)?
            .ok_or_else(|| not_found("objective_plan", plan_id.as_str()))?;
        if plan_objective_id != artifact.objective_id.as_str() {
            return Err(HarnessError::Conflict(format!(
                "artifact {} plan {} does not belong to objective {}",
                artifact.id, plan_id, artifact.objective_id
            )));
        }
    }
    conn.execute(
        "INSERT INTO objective_artifacts
         (id, objective_id, plan_id, planner_exchange_id, kind, path, sha256, byte_len, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            artifact.id.as_str(),
            artifact.objective_id.as_str(),
            artifact.plan_id.as_ref().map(ObjectivePlanId::as_str),
            artifact
                .planner_exchange_id
                .as_ref()
                .map(PlannerExchangeId::as_str),
            artifact.kind,
            artifact.path,
            artifact.sha256,
            artifact.byte_len as i64,
            artifact.created_at,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn insert_planner_exchange_row(conn: &Connection, exchange: &PlannerExchange) -> HarnessResult<()> {
    ensure_exists(
        conn,
        "objectives",
        exchange.objective_id.as_str(),
        "objective",
    )?;
    assert_planner_exchange_references(conn, exchange)?;
    conn.execute(
        "INSERT INTO planner_exchanges
         (id, objective_id, kind, ticket_id, model, system_prompt_version,
          request_objective_artifact_id, response_objective_artifact_id, status, error, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            exchange.id.as_str(),
            exchange.objective_id.as_str(),
            exchange.kind.as_str(),
            exchange.ticket_id.as_ref().map(TicketId::as_str),
            exchange.model,
            exchange.system_prompt_version,
            exchange
                .request_objective_artifact_id
                .as_ref()
                .map(ObjectiveArtifactId::as_str),
            exchange
                .response_objective_artifact_id
                .as_ref()
                .map(ObjectiveArtifactId::as_str),
            exchange.status,
            exchange.error,
            exchange.created_at,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn assert_initial_plan_exchange(exchange: &PlannerExchange) -> HarnessResult<()> {
    if exchange.kind != PlannerExchangeKind::InitialPlan {
        return Err(HarnessError::Conflict(
            "accepted objective plan bundle requires an initial_plan exchange".to_string(),
        ));
    }
    if exchange.status != "accepted" {
        return Err(HarnessError::Conflict(
            "accepted objective plan bundle requires an accepted exchange".to_string(),
        ));
    }
    if exchange.ticket_id.is_some() {
        return Err(HarnessError::Conflict(
            "accepted objective plan bundle exchange must not reference a ticket".to_string(),
        ));
    }
    Ok(())
}

fn insert_objective_message_row(
    conn: &Connection,
    message: &NewObjectiveMessage,
) -> HarnessResult<ObjectiveMessage> {
    ensure_exists(
        conn,
        "objectives",
        message.objective_id.as_str(),
        "objective",
    )?;
    let sequence: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), -1) + 1
             FROM objective_messages
             WHERE objective_id = ?1",
            params![message.objective_id.as_str()],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    conn.execute(
        "INSERT INTO objective_messages
         (id, objective_id, sequence, role, kind, content_objective_artifact_id, content_preview, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            message.id.as_str(),
            message.objective_id.as_str(),
            sequence,
            message.role,
            message.kind,
            message
                .content_objective_artifact_id
                .as_ref()
                .map(ObjectiveArtifactId::as_str),
            message.content_preview,
            message.created_at,
        ],
    )
    .map_err(sql_err)?;
    Ok(ObjectiveMessage {
        id: message.id.clone(),
        objective_id: message.objective_id.clone(),
        sequence: sequence as u32,
        role: message.role.clone(),
        kind: message.kind.clone(),
        content_objective_artifact_id: message.content_objective_artifact_id.clone(),
        content_preview: message.content_preview.clone(),
        created_at: message.created_at.clone(),
    })
}

fn insert_objective_event_row(conn: &Connection, event: &ObjectiveEvent) -> HarnessResult<()> {
    conn.execute(
        "INSERT INTO objective_events (id, objective_id, event_type, message, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.id.as_str(),
            event.objective_id.as_str(),
            event.event_type,
            event.message,
            event.payload_json,
            event.created_at,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn assert_objective_task_link(
    conn: &Connection,
    objective_id: &ObjectiveId,
    plan_id: &ObjectivePlanId,
    task_id: &crate::domain::TaskId,
) -> HarnessResult<()> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM objective_tasks
                WHERE objective_id = ?1 AND plan_id = ?2 AND task_id = ?3
             )",
            params![objective_id.as_str(), plan_id.as_str(), task_id.as_str()],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if exists {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "task {} is not linked to objective {} plan {}",
            task_id, objective_id, plan_id
        )))
    }
}

fn assert_message_matches_objective(
    conn: &Connection,
    message: &NewObjectiveMessage,
    objective_id: &ObjectiveId,
) -> HarnessResult<()> {
    if &message.objective_id != objective_id {
        return Err(HarnessError::Conflict(format!(
            "message {} does not belong to objective {}",
            message.id, objective_id
        )));
    }
    if let Some(artifact_id) = &message.content_objective_artifact_id {
        assert_artifact_id_belongs_to_objective(conn, artifact_id, objective_id)?;
    }
    Ok(())
}

fn assert_event_matches_objective(
    event: &ObjectiveEvent,
    objective_id: &ObjectiveId,
) -> HarnessResult<()> {
    if &event.objective_id == objective_id {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "event {} does not belong to objective {}",
            event.id, objective_id
        )))
    }
}

fn assert_planner_exchange_references(
    conn: &Connection,
    exchange: &PlannerExchange,
) -> HarnessResult<()> {
    if let Some(ticket_id) = &exchange.ticket_id {
        assert_ticket_belongs_to_objective(conn, ticket_id, &exchange.objective_id)?;
    }
    if let Some(artifact_id) = &exchange.request_objective_artifact_id {
        assert_artifact_id_belongs_to_objective(conn, artifact_id, &exchange.objective_id)?;
    }
    if let Some(artifact_id) = &exchange.response_objective_artifact_id {
        assert_artifact_id_belongs_to_objective(conn, artifact_id, &exchange.objective_id)?;
    }
    Ok(())
}

fn assert_artifact_id_belongs_to_objective(
    conn: &Connection,
    artifact_id: &ObjectiveArtifactId,
    objective_id: &ObjectiveId,
) -> HarnessResult<()> {
    let belongs: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM objective_artifacts
                WHERE id = ?1 AND objective_id = ?2
             )",
            params![artifact_id.as_str(), objective_id.as_str()],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if belongs {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "artifact {} does not belong to objective {}",
            artifact_id, objective_id
        )))
    }
}

fn assert_ticket_belongs_to_objective(
    conn: &Connection,
    ticket_id: &TicketId,
    objective_id: &ObjectiveId,
) -> HarnessResult<()> {
    let belongs: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM tickets ticket
                JOIN objective_tasks ot ON ot.task_id = ticket.task_id
                WHERE ticket.id = ?1 AND ot.objective_id = ?2
             )",
            params![ticket_id.as_str(), objective_id.as_str()],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if belongs {
        Ok(())
    } else {
        ensure_exists(conn, "tickets", ticket_id.as_str(), "ticket")?;
        Err(HarnessError::Conflict(format!(
            "ticket {} does not belong to objective {}",
            ticket_id, objective_id
        )))
    }
}

fn assert_ticket_resolution_exchange_matches_attempt(
    conn: &Connection,
    exchange_id: &PlannerExchangeId,
    attempt: &ObjectiveResolverAttempt,
) -> HarnessResult<()> {
    let exchange = conn
        .query_row(
            "SELECT * FROM planner_exchanges WHERE id = ?1",
            params![exchange_id.as_str()],
            row_to_planner_exchange,
        )
        .optional()
        .map_err(sql_err)?
        .ok_or_else(|| not_found("planner_exchange", exchange_id.as_str()))?;
    if exchange.objective_id != attempt.objective_id
        || exchange.kind != PlannerExchangeKind::TicketResolution
        || exchange.ticket_id.as_ref() != Some(&attempt.ticket_id)
    {
        return Err(HarnessError::Conflict(format!(
            "planner exchange {} does not match resolver attempt {}",
            exchange_id, attempt.id
        )));
    }
    Ok(())
}

fn insert_objective_task_row(
    conn: &Connection,
    objective_task: &ObjectiveTask,
) -> HarnessResult<()> {
    conn.execute(
        "INSERT INTO objective_tasks
         (objective_id, task_id, plan_id, task_key, parallel_group, owned_paths_json, sequence,
          worker_attempt_budget, worker_attempts_used)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            objective_task.objective_id.as_str(),
            objective_task.task_id.as_str(),
            objective_task.plan_id.as_str(),
            objective_task.task_key,
            objective_task.parallel_group,
            objective_task.owned_paths_json,
            objective_task.sequence as i64,
            objective_task.worker_attempt_budget as i64,
            objective_task.worker_attempts_used as i64,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn insert_objective_task_validation_command_row(
    conn: &Connection,
    command: &super::ObjectiveTaskValidationCommand,
) -> HarnessResult<()> {
    conn.execute(
        "INSERT INTO objective_task_validation_commands
         (id, objective_id, task_id, command, review_status, review_reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            command.id.as_str(),
            command.objective_id.as_str(),
            command.task_id.as_str(),
            command.command,
            command.review_status.as_str(),
            command.review_reason,
            command.created_at,
        ],
    )
    .map_err(sql_err)?;
    Ok(())
}

fn get_resolver_attempt_with_conn(
    conn: &Connection,
    attempt_id: &ObjectiveResolverAttemptId,
) -> HarnessResult<ObjectiveResolverAttempt> {
    conn.query_row(
        "SELECT * FROM objective_ticket_resolver_attempts WHERE id = ?1",
        params![attempt_id.as_str()],
        row_to_resolver_attempt,
    )
    .optional()
    .map_err(sql_err)?
    .ok_or_else(|| not_found("objective_resolver_attempt", attempt_id.as_str()))
}

fn update_monitor_lease(
    store: &SqliteTaskStore,
    objective_id: &ObjectiveId,
    owner: &str,
    refresh_only: bool,
) -> HarnessResult<ObjectiveMonitorLease> {
    let now = super::current_unix_secs();
    let expires_at = (now + store.lease_ttl_secs).to_string();
    let mut conn = store.lock_conn()?;
    let tx = conn.transaction().map_err(sql_err)?;
    ensure_exists(&tx, "objectives", objective_id.as_str(), "objective")?;
    let rows = if refresh_only {
        tx.execute(
            "UPDATE objectives
             SET monitor_lease_owner = ?2, monitor_lease_expires_at = ?3, updated_at = ?4
             WHERE id = ?1 AND monitor_lease_owner = ?2
               AND monitor_lease_expires_at IS NOT NULL
               AND CAST(monitor_lease_expires_at AS INTEGER) > ?5",
            params![
                objective_id.as_str(),
                owner,
                expires_at,
                now.to_string(),
                now
            ],
        )
        .map_err(sql_err)?
    } else {
        tx.execute(
            "UPDATE objectives
             SET monitor_lease_owner = ?2, monitor_lease_expires_at = ?3, updated_at = ?4
             WHERE id = ?1
               AND (monitor_lease_owner IS NULL
                    OR monitor_lease_expires_at IS NULL
                    OR CAST(monitor_lease_expires_at AS INTEGER) <= ?5)",
            params![
                objective_id.as_str(),
                owner,
                expires_at,
                now.to_string(),
                now
            ],
        )
        .map_err(sql_err)?
    };
    if rows == 0 {
        return Err(HarnessError::Conflict(format!(
            "objective {} monitor lease is not available to {owner}",
            objective_id
        )));
    }
    tx.commit().map_err(sql_err)?;
    Ok(ObjectiveMonitorLease {
        objective_id: objective_id.clone(),
        owner: owner.to_string(),
        expires_at,
    })
}

fn row_to_objective_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectiveArtifact> {
    Ok(ObjectiveArtifact {
        id: parse_id(row.get(0)?, 0)?,
        objective_id: parse_id(row.get(1)?, 1)?,
        plan_id: parse_optional_id(row.get(2)?, 2)?,
        planner_exchange_id: parse_optional_id(row.get(3)?, 3)?,
        kind: row.get(4)?,
        path: row.get(5)?,
        sha256: row.get(6)?,
        byte_len: row.get::<_, i64>(7)? as u64,
        created_at: row.get(8)?,
    })
}

fn row_to_planner_exchange(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlannerExchange> {
    Ok(PlannerExchange {
        id: parse_id(row.get(0)?, 0)?,
        objective_id: parse_id(row.get(1)?, 1)?,
        kind: parse_status(row.get::<_, String>(2)?, 2)?,
        ticket_id: parse_optional_id(row.get(3)?, 3)?,
        model: row.get(4)?,
        system_prompt_version: row.get(5)?,
        request_objective_artifact_id: parse_optional_id(row.get(6)?, 6)?,
        response_objective_artifact_id: parse_optional_id(row.get(7)?, 7)?,
        status: row.get(8)?,
        error: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn row_to_objective_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectiveMessage> {
    Ok(ObjectiveMessage {
        id: parse_id(row.get(0)?, 0)?,
        objective_id: parse_id(row.get(1)?, 1)?,
        sequence: row.get::<_, i64>(2)? as u32,
        role: row.get(3)?,
        kind: row.get(4)?,
        content_objective_artifact_id: parse_optional_id(row.get(5)?, 5)?,
        content_preview: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn row_to_objective_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectiveEvent> {
    Ok(ObjectiveEvent {
        id: parse_id(row.get(0)?, 0)?,
        objective_id: parse_id(row.get(1)?, 1)?,
        event_type: row.get(2)?,
        message: row.get(3)?,
        payload_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn row_to_resolver_attempt(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectiveResolverAttempt> {
    Ok(ObjectiveResolverAttempt {
        id: parse_id(row.get(0)?, 0)?,
        objective_id: parse_id(row.get(1)?, 1)?,
        ticket_id: parse_id(row.get(2)?, 2)?,
        attempt: row.get::<_, i64>(3)? as u32,
        status: row.get(4)?,
        lease_owner: row.get(5)?,
        lease_expires_at: row.get(6)?,
        planner_exchange_id: parse_optional_id(row.get(7)?, 7)?,
        last_error: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn assert_exchange_matches_objective(
    exchange: &PlannerExchange,
    objective_id: &ObjectiveId,
) -> HarnessResult<()> {
    if &exchange.objective_id == objective_id {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "planner exchange {} does not belong to objective {}",
            exchange.id, objective_id
        )))
    }
}

fn assert_artifact_matches_objective(
    artifact: &ObjectiveArtifact,
    objective_id: &ObjectiveId,
) -> HarnessResult<()> {
    if &artifact.objective_id == objective_id {
        Ok(())
    } else {
        Err(HarnessError::Conflict(format!(
            "artifact {} does not belong to objective {}",
            artifact.id, objective_id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ObjectiveAcceptanceStatus, ObjectiveValidationCommandSource,
        ObjectiveValidationReviewStatus, PlannerExchangeKind, Run, RunId, RunStatus, Task, TaskId,
        Ticket, TicketStatus,
    };
    use crate::state::{
        NewGeneratedTask, NewObjectiveMessage, NewObjectiveResolverAttempt,
        NewObjectiveTaskDependency, ObjectiveAcceptanceCriterion, ObjectiveArtifact,
        ObjectiveEvent, ObjectivePlan, ObjectivePlanBundle, ObjectiveStatusUpdate, ObjectiveStore,
        ObjectiveTaskValidationCommand, ObjectiveValidationCommand, PlannerExchange,
        RejectedPlannerExchange, TaskStore,
    };

    const OBJECTIVE_1: &str = "objective_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const OBJECTIVE_2: &str = "objective_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const OBJECTIVE_3: &str = "objective_01ARZ3NDEKTSV4RRFFQ69G5FAX";
    const PLAN_1: &str = "plan_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const PLAN_2: &str = "plan_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const EXCHANGE_1: &str = "planner_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const EXCHANGE_2: &str = "planner_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const ARTIFACT_1: &str = "obj_art_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const ARTIFACT_2: &str = "obj_art_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const MESSAGE_1: &str = "omsg_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const MESSAGE_2: &str = "omsg_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const EVENT_1: &str = "oevent_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const CRITERION_1: &str = "criterion_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const VALIDATION_1: &str = "validation_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const VALIDATION_2: &str = "validation_01ARZ3NDEKTSV4RRFFQ69G5FAW";
    const TASK_0: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FA0";
    const TASK_1: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FA1";
    const TASK_2: &str = "task_01ARZ3NDEKTSV4RRFFQ69G5FA2";
    const RUN_1: &str = "run_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const TICKET_1: &str = "ticket_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RESOLVER_1: &str = "resolver_01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const RESOLVER_2: &str = "resolver_01ARZ3NDEKTSV4RRFFQ69G5FAW";

    #[test]
    fn objective_store_crud_round_trips_and_updates_status() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let objective = sample_objective(
            OBJECTIVE_1,
            ObjectiveStatus::Planning,
            "2026-01-01T00:00:00Z",
        );

        store.insert_objective(objective.clone()).unwrap();
        assert_eq!(store.get_objective(&objective.id).unwrap(), objective);

        let updated = store
            .update_objective_status(
                &objective.id,
                Some(ObjectiveStatus::Planning),
                ObjectiveStatusUpdate {
                    status: Some(ObjectiveStatus::Ready),
                    summary: Some("Plan accepted".to_string()),
                    updated_at: Some("2026-01-01T00:00:05Z".to_string()),
                    ..ObjectiveStatusUpdate::default()
                },
            )
            .unwrap();

        assert_eq!(updated.status, ObjectiveStatus::Ready);
        assert_eq!(updated.summary, "Plan accepted");
        assert_eq!(updated.updated_at, "2026-01-01T00:00:05Z");
        assert!(
            store
                .update_objective_status(
                    &objective.id,
                    Some(ObjectiveStatus::Planning),
                    ObjectiveStatusUpdate {
                        status: Some(ObjectiveStatus::Running),
                        ..ObjectiveStatusUpdate::default()
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn objective_store_list_filters_by_status_with_stable_ordering() {
        let store = SqliteTaskStore::in_memory().unwrap();
        let ready_later =
            sample_objective(OBJECTIVE_2, ObjectiveStatus::Ready, "2026-01-01T00:00:02Z");
        let planning = sample_objective(
            OBJECTIVE_3,
            ObjectiveStatus::Planning,
            "2026-01-01T00:00:03Z",
        );
        let ready_first =
            sample_objective(OBJECTIVE_1, ObjectiveStatus::Ready, "2026-01-01T00:00:01Z");

        store.insert_objective(ready_later).unwrap();
        store.insert_objective(planning).unwrap();
        store.insert_objective(ready_first).unwrap();

        let ready = store.list_objectives(Some(ObjectiveStatus::Ready)).unwrap();
        assert_eq!(
            ready
                .iter()
                .map(|objective| objective.id.as_str())
                .collect::<Vec<_>>(),
            vec![OBJECTIVE_1, OBJECTIVE_2]
        );

        let all = store.list_objectives(None).unwrap();
        assert_eq!(
            all.iter()
                .map(|objective| objective.id.as_str())
                .collect::<Vec<_>>(),
            vec![OBJECTIVE_1, OBJECTIVE_2, OBJECTIVE_3]
        );
    }

    #[test]
    fn objective_plan_bundle_success_inserts_generated_state() {
        let store = seeded_objective_store();
        let result = store
            .create_objective_plan_bundle(&objective_id(), sample_bundle())
            .unwrap();

        assert_eq!(result.objective.status, ObjectiveStatus::Ready);
        assert_eq!(
            result.objective.active_plan_id.as_ref().unwrap().as_str(),
            PLAN_1
        );
        assert_eq!(result.tasks.len(), 2);
        assert_eq!(result.messages[0].sequence, 0);
        assert_eq!(result.messages[1].sequence, 1);

        assert_eq!(store.list_tasks(None).unwrap().len(), 2);
        assert_eq!(
            store
                .list_validation_commands(&TaskId::parse(TASK_1).unwrap())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .list_validation_commands(&TaskId::parse(TASK_2).unwrap())
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            store
                .list_objective_artifacts(&objective_id())
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.list_planner_exchanges(&objective_id()).unwrap().len(),
            1
        );
        assert_eq!(count_rows(&store, "objective_task_dependencies"), 1);
        assert_eq!(count_rows(&store, "objective_task_validation_commands"), 1);
        assert_eq!(count_rows(&store, "objective_events"), 1);
    }

    #[test]
    fn objective_plan_bundle_failure_rolls_back_inserted_rows() {
        let store = seeded_objective_store();
        let mut bundle = sample_bundle();
        bundle.dependencies.push(NewObjectiveTaskDependency {
            task_id: TaskId::parse(TASK_2).unwrap(),
            depends_on_task_id: TaskId::parse("task_01ARZ3NDEKTSV4RRFFQ69G5FA3").unwrap(),
        });

        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );

        assert_eq!(count_rows(&store, "objective_plans"), 0);
        assert_eq!(count_rows(&store, "tasks"), 0);
        assert_eq!(count_rows(&store, "planner_exchanges"), 0);
        assert_eq!(count_rows(&store, "objective_messages"), 0);
        assert_eq!(
            store.get_objective(&objective_id()).unwrap().status,
            ObjectiveStatus::Planning
        );
    }

    #[test]
    fn objective_plan_bundle_rejects_cross_objective_state() {
        let store = seeded_objective_store();
        store
            .insert_objective(sample_objective(
                OBJECTIVE_2,
                ObjectiveStatus::Planning,
                "2026-01-01T00:00:09Z",
            ))
            .unwrap();

        let mut bundle = sample_bundle();
        bundle.messages[0].objective_id = ObjectiveId::parse(OBJECTIVE_2).unwrap();
        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );
        assert_eq!(count_rows(&store, "objective_plans"), 0);

        let mut bundle = sample_bundle();
        bundle.events[0].objective_id = ObjectiveId::parse(OBJECTIVE_2).unwrap();
        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );
        assert_eq!(count_rows(&store, "objective_plans"), 0);

        let external_task = generated_task(TASK_0, "external");
        store
            .insert_task(external_task.clone(), Vec::new())
            .unwrap();
        let mut bundle = sample_bundle();
        bundle.dependencies = vec![NewObjectiveTaskDependency {
            task_id: TaskId::parse(TASK_2).unwrap(),
            depends_on_task_id: external_task.id,
        }];
        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );
        assert_eq!(count_rows(&store, "objective_plans"), 0);
    }

    #[test]
    fn objective_plan_bundle_requires_accepted_initial_plan_exchange() {
        let store = seeded_objective_store();
        let mut bundle = sample_bundle();
        bundle.exchange.kind = PlannerExchangeKind::TicketResolution;
        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );

        let mut bundle = sample_bundle();
        bundle.exchange.status = "failed".to_string();
        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );

        let mut bundle = sample_bundle();
        bundle.exchange.ticket_id = Some(TicketId::parse(TICKET_1).unwrap());
        assert!(
            store
                .create_objective_plan_bundle(&objective_id(), bundle)
                .is_err()
        );
    }

    #[test]
    fn rejected_objective_plan_creates_no_generated_tasks() {
        let store = seeded_objective_store();
        let artifact = sample_artifact(ARTIFACT_1, None, Some(EXCHANGE_2));
        let exchange = PlannerExchange {
            id: PlannerExchangeId::parse(EXCHANGE_2).unwrap(),
            objective_id: objective_id(),
            kind: PlannerExchangeKind::InitialPlan,
            ticket_id: None,
            model: "planner".to_string(),
            system_prompt_version: "v1".to_string(),
            request_objective_artifact_id: Some(artifact.id.clone()),
            response_objective_artifact_id: None,
            status: "rejected".to_string(),
            error: Some("invalid planner response".to_string()),
            created_at: "2026-01-01T00:00:04Z".to_string(),
        };

        store
            .reject_objective_plan(
                &objective_id(),
                RejectedPlannerExchange {
                    artifacts: vec![artifact],
                    exchange,
                    messages: vec![sample_message(MESSAGE_1, "planner", Some(ARTIFACT_1))],
                },
                sample_event("objective.plan_rejected"),
            )
            .unwrap();

        assert_eq!(count_rows(&store, "tasks"), 0);
        assert_eq!(count_rows(&store, "objective_tasks"), 0);
        assert_eq!(
            store.get_objective(&objective_id()).unwrap().status,
            ObjectiveStatus::Failed
        );
        assert_eq!(
            store.list_planner_exchanges(&objective_id()).unwrap().len(),
            1
        );
    }

    #[test]
    fn objective_artifacts_events_and_ticket_resolution_exchanges_round_trip() {
        let store = seeded_objective_with_ticket();
        let artifact = sample_artifact(ARTIFACT_1, None, Some(EXCHANGE_2));
        store.insert_objective_artifact(artifact.clone()).unwrap();
        store
            .insert_planner_exchange(PlannerExchange {
                id: PlannerExchangeId::parse(EXCHANGE_2).unwrap(),
                objective_id: objective_id(),
                kind: PlannerExchangeKind::TicketResolution,
                ticket_id: Some(TicketId::parse(TICKET_1).unwrap()),
                model: "resolver".to_string(),
                system_prompt_version: "v1".to_string(),
                request_objective_artifact_id: Some(artifact.id),
                response_objective_artifact_id: None,
                status: "accepted".to_string(),
                error: None,
                created_at: "2026-01-01T00:00:06Z".to_string(),
            })
            .unwrap();
        store
            .insert_objective_event(sample_event("objective.ticket_resolution"))
            .unwrap();

        let exchanges = store.list_planner_exchanges(&objective_id()).unwrap();
        assert_eq!(exchanges[0].kind, PlannerExchangeKind::TicketResolution);
        assert_eq!(exchanges[0].ticket_id.as_ref().unwrap().as_str(), TICKET_1);
        assert_eq!(
            store
                .list_objective_artifacts(&objective_id())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.list_objective_events(&objective_id()).unwrap().len(),
            1
        );
    }

    #[test]
    fn objective_message_insert_requires_artifact_ownership() {
        let store = seeded_objective_store();
        store
            .insert_objective(sample_objective(
                OBJECTIVE_2,
                ObjectiveStatus::Planning,
                "2026-01-01T00:00:09Z",
            ))
            .unwrap();
        store
            .insert_objective_artifact(sample_artifact(ARTIFACT_1, None, None))
            .unwrap();

        assert!(
            store
                .insert_objective_message(NewObjectiveMessage {
                    id: MESSAGE_1.parse().unwrap(),
                    objective_id: ObjectiveId::parse(OBJECTIVE_2).unwrap(),
                    role: "planner".to_string(),
                    kind: "preview".to_string(),
                    content_objective_artifact_id: Some(
                        ObjectiveArtifactId::parse(ARTIFACT_1).unwrap()
                    ),
                    content_preview: "cross objective artifact".to_string(),
                    created_at: "2026-01-01T00:00:03Z".to_string(),
                })
                .is_err()
        );
    }

    #[test]
    fn objective_exchange_and_resolver_attempt_require_ticket_ownership() {
        let store = seeded_objective_with_ticket();
        store
            .insert_objective(sample_objective(
                OBJECTIVE_2,
                ObjectiveStatus::Planning,
                "2026-01-01T00:00:09Z",
            ))
            .unwrap();

        assert!(
            store
                .insert_planner_exchange(PlannerExchange {
                    id: PlannerExchangeId::parse(EXCHANGE_2).unwrap(),
                    objective_id: ObjectiveId::parse(OBJECTIVE_2).unwrap(),
                    kind: PlannerExchangeKind::TicketResolution,
                    ticket_id: Some(TicketId::parse(TICKET_1).unwrap()),
                    model: "resolver".to_string(),
                    system_prompt_version: "v1".to_string(),
                    request_objective_artifact_id: None,
                    response_objective_artifact_id: None,
                    status: "accepted".to_string(),
                    error: None,
                    created_at: "2026-01-01T00:00:06Z".to_string(),
                })
                .is_err()
        );
        assert!(
            store
                .create_resolver_attempt(NewObjectiveResolverAttempt {
                    id: ObjectiveResolverAttemptId::parse(RESOLVER_1).unwrap(),
                    objective_id: ObjectiveId::parse(OBJECTIVE_2).unwrap(),
                    ticket_id: TicketId::parse(TICKET_1).unwrap(),
                    attempt: 1,
                    created_at: "2026-01-01T00:00:06Z".to_string(),
                })
                .is_err()
        );
    }

    #[test]
    fn objective_scheduling_respects_dependencies_and_deterministic_ordering() {
        let store = seeded_objective_store();
        let mut bundle = sample_bundle();
        bundle.generated_tasks[0].task.id = TaskId::parse(TASK_1).unwrap();
        bundle.generated_tasks[0].task_key = "late_dependency".to_string();
        bundle.generated_tasks[0].sequence = 5;
        bundle.generated_tasks[1].task.id = TaskId::parse(TASK_0).unwrap();
        bundle.generated_tasks[1].task_key = "early_blocked".to_string();
        bundle.generated_tasks[1].sequence = 0;
        bundle.generated_tasks[1].reviewed_validation_commands[0].task_id =
            TaskId::parse(TASK_0).unwrap();
        bundle.dependencies = vec![NewObjectiveTaskDependency {
            task_id: TaskId::parse(TASK_0).unwrap(),
            depends_on_task_id: TaskId::parse(TASK_1).unwrap(),
        }];
        store
            .create_objective_plan_bundle(&objective_id(), bundle)
            .unwrap();

        let first = store
            .next_ready_objective_task(&objective_id())
            .unwrap()
            .unwrap();
        assert_eq!(first.id.as_str(), TASK_1);

        {
            let conn = store.lock_conn().unwrap();
            conn.execute(
                "UPDATE tasks SET status = 'complete' WHERE id = ?1",
                params![TASK_1],
            )
            .unwrap();
        }

        let second = store
            .next_ready_objective_task(&objective_id())
            .unwrap()
            .unwrap();
        assert_eq!(second.id.as_str(), TASK_0);
    }

    #[test]
    fn objective_scheduling_uses_only_active_plan_tasks() {
        let store = seeded_objective_store();
        store
            .create_objective_plan_bundle(&objective_id(), sample_bundle())
            .unwrap();
        store
            .insert_task(
                generated_task(TASK_0, "stale inactive task"),
                vec!["cargo test".to_string()],
            )
            .unwrap();
        {
            let conn = store.lock_conn().unwrap();
            conn.execute(
                "INSERT INTO objective_plans (id, objective_id, version, summary, created_at)
                 VALUES (?1, ?2, 2, 'Inactive plan', '2026-01-01T00:00:05Z')",
                params![PLAN_2, OBJECTIVE_1],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO objective_tasks
                 (objective_id, task_id, plan_id, task_key, parallel_group, owned_paths_json,
                  sequence, worker_attempt_budget, worker_attempts_used)
                 VALUES (?1, ?2, ?3, 'inactive_first', 'main', '[]', 0, 3, 0)",
                params![OBJECTIVE_1, TASK_0, PLAN_2],
            )
            .unwrap();
        }

        let next = store
            .next_ready_objective_task(&objective_id())
            .unwrap()
            .unwrap();
        assert_eq!(next.id.as_str(), TASK_1);
    }

    #[test]
    fn objective_scheduling_uses_task_id_as_stable_tiebreaker() {
        let store = seeded_objective_store();
        let mut bundle = sample_bundle();
        bundle.generated_tasks[0].task.id = TaskId::parse(TASK_2).unwrap();
        bundle.generated_tasks[0].task_key = "second_by_id".to_string();
        bundle.generated_tasks[0].sequence = 0;
        bundle.generated_tasks[1].task.id = TaskId::parse(TASK_0).unwrap();
        bundle.generated_tasks[1].task_key = "first_by_id".to_string();
        bundle.generated_tasks[1].sequence = 0;
        bundle.generated_tasks[1].reviewed_validation_commands[0].task_id =
            TaskId::parse(TASK_0).unwrap();
        bundle.dependencies.clear();
        store
            .create_objective_plan_bundle(&objective_id(), bundle)
            .unwrap();

        let next = store
            .next_ready_objective_task(&objective_id())
            .unwrap()
            .unwrap();
        assert_eq!(next.id.as_str(), TASK_0);
    }

    #[test]
    fn objective_resolver_attempt_lease_prevents_duplicate_work() {
        let store = seeded_objective_with_ticket();
        let first = store
            .create_resolver_attempt(NewObjectiveResolverAttempt {
                id: ObjectiveResolverAttemptId::parse(RESOLVER_1).unwrap(),
                objective_id: objective_id(),
                ticket_id: TicketId::parse(TICKET_1).unwrap(),
                attempt: 1,
                created_at: "2026-01-01T00:00:06Z".to_string(),
            })
            .unwrap();
        store
            .create_resolver_attempt(NewObjectiveResolverAttempt {
                id: ObjectiveResolverAttemptId::parse(RESOLVER_2).unwrap(),
                objective_id: objective_id(),
                ticket_id: TicketId::parse(TICKET_1).unwrap(),
                attempt: 2,
                created_at: "2026-01-01T00:00:07Z".to_string(),
            })
            .unwrap();

        let leased = store
            .acquire_resolver_attempt_lease(&first.id, "resolver-a")
            .unwrap();
        assert_eq!(leased.status, "resolving");
        assert_eq!(leased.lease_owner.as_deref(), Some("resolver-a"));
        assert!(
            store
                .acquire_resolver_attempt_lease(
                    &ObjectiveResolverAttemptId::parse(RESOLVER_2).unwrap(),
                    "resolver-b"
                )
                .is_err()
        );

        let released = store
            .release_resolver_attempt_lease(&first.id, "resolver-a", "failed", None, Some("nope"))
            .unwrap();
        assert_eq!(released.status, "failed");
        assert_eq!(released.lease_owner, None);
        assert!(
            store
                .acquire_resolver_attempt_lease(
                    &ObjectiveResolverAttemptId::parse(RESOLVER_2).unwrap(),
                    "resolver-b"
                )
                .is_ok()
        );
    }

    #[test]
    fn objective_resolver_attempt_release_rejects_expired_lease() {
        let store = seeded_objective_with_ticket();
        let attempt = store
            .create_resolver_attempt(NewObjectiveResolverAttempt {
                id: ObjectiveResolverAttemptId::parse(RESOLVER_1).unwrap(),
                objective_id: objective_id(),
                ticket_id: TicketId::parse(TICKET_1).unwrap(),
                attempt: 1,
                created_at: "2026-01-01T00:00:06Z".to_string(),
            })
            .unwrap();
        store
            .acquire_resolver_attempt_lease(&attempt.id, "resolver-a")
            .unwrap();
        {
            let conn = store.lock_conn().unwrap();
            conn.execute(
                "UPDATE objective_ticket_resolver_attempts SET lease_expires_at = '0' WHERE id = ?1",
                params![attempt.id.as_str()],
            )
            .unwrap();
        }
        assert!(
            store
                .release_resolver_attempt_lease(&attempt.id, "resolver-a", "resolved", None, None)
                .is_err()
        );
        let attempt = store.acquire_resolver_attempt_lease(&attempt.id, "resolver-b");
        assert!(attempt.is_ok());
    }

    #[test]
    fn objective_resolver_release_validates_exchange_ownership_and_kind() {
        let store = seeded_objective_with_ticket();
        let attempt = store
            .create_resolver_attempt(NewObjectiveResolverAttempt {
                id: ObjectiveResolverAttemptId::parse(RESOLVER_1).unwrap(),
                objective_id: objective_id(),
                ticket_id: TicketId::parse(TICKET_1).unwrap(),
                attempt: 1,
                created_at: "2026-01-01T00:00:06Z".to_string(),
            })
            .unwrap();
        store
            .acquire_resolver_attempt_lease(&attempt.id, "resolver-a")
            .unwrap();
        store
            .insert_planner_exchange(PlannerExchange {
                id: PlannerExchangeId::parse(EXCHANGE_2).unwrap(),
                objective_id: objective_id(),
                kind: PlannerExchangeKind::InitialPlan,
                ticket_id: None,
                model: "planner".to_string(),
                system_prompt_version: "v1".to_string(),
                request_objective_artifact_id: None,
                response_objective_artifact_id: None,
                status: "accepted".to_string(),
                error: None,
                created_at: "2026-01-01T00:00:06Z".to_string(),
            })
            .unwrap();

        assert!(
            store
                .release_resolver_attempt_lease(
                    &attempt.id,
                    "resolver-a",
                    "resolved",
                    Some(&PlannerExchangeId::parse(EXCHANGE_2).unwrap()),
                    None,
                )
                .is_err()
        );
    }

    #[test]
    fn objective_monitor_lease_conflicts_refreshes_and_releases() {
        let store = seeded_objective_store();
        let lease = store
            .acquire_objective_monitor_lease(&objective_id(), "monitor-a")
            .unwrap();
        assert_eq!(lease.owner, "monitor-a");
        assert!(
            store
                .acquire_objective_monitor_lease(&objective_id(), "monitor-b")
                .is_err()
        );
        assert!(
            store
                .refresh_objective_monitor_lease(&objective_id(), "monitor-b")
                .is_err()
        );
        assert!(
            store
                .refresh_objective_monitor_lease(&objective_id(), "monitor-a")
                .is_ok()
        );
        store
            .release_objective_monitor_lease(&objective_id(), "monitor-a")
            .unwrap();
        assert!(
            store
                .acquire_objective_monitor_lease(&objective_id(), "monitor-b")
                .is_ok()
        );
    }

    fn sample_objective(id: &str, status: ObjectiveStatus, created_at: &str) -> Objective {
        Objective {
            id: ObjectiveId::parse(id).unwrap(),
            title: format!("Objective {id}"),
            prompt: "Build the requested project".to_string(),
            summary: "Initial shell".to_string(),
            status,
            planner_model: Some("gpt-planner".to_string()),
            worker_model: Some("local-worker".to_string()),
            ticket_model: Some("gpt-ticket".to_string()),
            active_plan_id: None,
            monitor_lease_owner: None,
            monitor_lease_expires_at: None,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
        }
    }

    fn seeded_objective_store() -> SqliteTaskStore {
        let store = SqliteTaskStore::in_memory().unwrap();
        store
            .insert_objective(sample_objective(
                OBJECTIVE_1,
                ObjectiveStatus::Planning,
                "2026-01-01T00:00:00Z",
            ))
            .unwrap();
        store
    }

    fn seeded_objective_with_ticket() -> SqliteTaskStore {
        let store = seeded_objective_store();
        let task = generated_task(TASK_1, "ticket task");
        store
            .insert_task(task.clone(), vec!["cargo test".to_string()])
            .unwrap();
        {
            let conn = store.lock_conn().unwrap();
            conn.execute(
                "INSERT INTO objective_plans (id, objective_id, version, summary, created_at)
                 VALUES (?1, ?2, 1, 'Ticket plan', '2026-01-01T00:00:01Z')",
                params![PLAN_1, OBJECTIVE_1],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO objective_tasks
                 (objective_id, task_id, plan_id, task_key, parallel_group, owned_paths_json,
                  sequence, worker_attempt_budget, worker_attempts_used)
                 VALUES (?1, ?2, ?3, 'ticket_task', 'main', '[]', 0, 3, 0)",
                params![OBJECTIVE_1, task.id.as_str(), PLAN_1],
            )
            .unwrap();
        }
        store.acquire_task_lease(&task.id, "ticket-owner").unwrap();
        store
            .insert_run(
                Run {
                    id: RunId::parse(RUN_1).unwrap(),
                    task_id: task.id.clone(),
                    parent_run_id: None,
                    status: RunStatus::Stuck,
                    repo_root: "/repo".to_string(),
                    base_ref: Some("main".to_string()),
                    base_commit: "abcdef".to_string(),
                    dirty_state_summary: None,
                    current_phase: Some("validation".to_string()),
                    escalation_cycle: 0,
                    started_at: "2026-01-01T00:00:02Z".to_string(),
                    finished_at: None,
                    final_diff_path: None,
                    last_error: None,
                },
                "ticket-owner",
            )
            .unwrap();
        store
            .insert_ticket(
                Ticket {
                    id: TicketId::parse(TICKET_1).unwrap(),
                    task_id: task.id,
                    run_id: RunId::parse(RUN_1).unwrap(),
                    status: TicketStatus::Open,
                    blocked_on: "validation".to_string(),
                    question: "What should change?".to_string(),
                    reason: "Tests failed".to_string(),
                    evidence_json: "{}".to_string(),
                    failure_fingerprint: "fp".to_string(),
                    created_at: "2026-01-01T00:00:05Z".to_string(),
                    resolved_at: None,
                },
                "ticket-owner",
            )
            .unwrap();
        store
    }

    fn sample_bundle() -> ObjectivePlanBundle {
        let plan = ObjectivePlan {
            id: ObjectivePlanId::parse(PLAN_1).unwrap(),
            objective_id: objective_id(),
            version: 1,
            summary: "Implement the objective".to_string(),
            created_at: "2026-01-01T00:00:01Z".to_string(),
        };
        let request = sample_artifact(ARTIFACT_1, Some(PLAN_1), Some(EXCHANGE_1));
        let response = sample_artifact(ARTIFACT_2, Some(PLAN_1), Some(EXCHANGE_1));
        ObjectivePlanBundle {
            objective_updated_at: "2026-01-01T00:00:03Z".to_string(),
            plan: plan.clone(),
            acceptance_criteria: vec![ObjectiveAcceptanceCriterion {
                id: CRITERION_1.parse().unwrap(),
                objective_id: objective_id(),
                plan_id: plan.id.clone(),
                description: "All tests pass".to_string(),
                status: ObjectiveAcceptanceStatus::Pending,
                last_evaluated_at: None,
            }],
            validation_commands: vec![ObjectiveValidationCommand {
                id: VALIDATION_1.parse().unwrap(),
                objective_id: objective_id(),
                plan_id: plan.id.clone(),
                command: "cargo test".to_string(),
                source: ObjectiveValidationCommandSource::Planner,
                review_status: ObjectiveValidationReviewStatus::Trusted,
                review_reason: None,
                created_at: "2026-01-01T00:00:01Z".to_string(),
            }],
            generated_tasks: vec![
                NewGeneratedTask {
                    task: generated_task(TASK_1, "first"),
                    task_key: "first".to_string(),
                    parallel_group: Some("main".to_string()),
                    owned_paths_json: r#"["src/state"]"#.to_string(),
                    sequence: 0,
                    worker_attempt_budget: 3,
                    trusted_validation_commands: vec!["cargo test first".to_string()],
                    reviewed_validation_commands: vec![],
                },
                NewGeneratedTask {
                    task: generated_task(TASK_2, "second"),
                    task_key: "second".to_string(),
                    parallel_group: Some("main".to_string()),
                    owned_paths_json: r#"["src/state"]"#.to_string(),
                    sequence: 1,
                    worker_attempt_budget: 3,
                    trusted_validation_commands: vec![],
                    reviewed_validation_commands: vec![ObjectiveTaskValidationCommand {
                        id: VALIDATION_2.parse().unwrap(),
                        objective_id: objective_id(),
                        task_id: TaskId::parse(TASK_2).unwrap(),
                        command: "rm -rf /".to_string(),
                        review_status: ObjectiveValidationReviewStatus::Rejected,
                        review_reason: Some("unsafe".to_string()),
                        created_at: "2026-01-01T00:00:01Z".to_string(),
                    }],
                },
            ],
            dependencies: vec![NewObjectiveTaskDependency {
                task_id: TaskId::parse(TASK_2).unwrap(),
                depends_on_task_id: TaskId::parse(TASK_1).unwrap(),
            }],
            artifacts: vec![request.clone(), response.clone()],
            exchange: PlannerExchange {
                id: PlannerExchangeId::parse(EXCHANGE_1).unwrap(),
                objective_id: objective_id(),
                kind: PlannerExchangeKind::InitialPlan,
                ticket_id: None,
                model: "planner".to_string(),
                system_prompt_version: "v1".to_string(),
                request_objective_artifact_id: Some(request.id),
                response_objective_artifact_id: Some(response.id),
                status: "accepted".to_string(),
                error: None,
                created_at: "2026-01-01T00:00:02Z".to_string(),
            },
            messages: vec![
                sample_message(MESSAGE_1, "user", None),
                sample_message(MESSAGE_2, "planner", Some(ARTIFACT_2)),
            ],
            events: vec![sample_event("objective.plan_accepted")],
        }
    }

    fn generated_task(id: &str, title: &str) -> Task {
        Task {
            id: TaskId::parse(id).unwrap(),
            title: title.to_string(),
            goal: format!("Complete {title}"),
            status: TaskStatus::Ready,
            repo_root: "/repo".to_string(),
            worktree_path: None,
            branch: None,
            base_ref: Some("main".to_string()),
            base_commit: Some("abcdef".to_string()),
            last_seen_head: None,
            max_attempts: 3,
            lease_owner: None,
            lease_acquired_at: None,
            lease_expires_at: None,
            heartbeat_at: None,
            lock_version: 0,
            created_at: "2026-01-01T00:00:01Z".to_string(),
            updated_at: "2026-01-01T00:00:01Z".to_string(),
        }
    }

    fn sample_artifact(
        id: &str,
        plan_id: Option<&str>,
        exchange_id: Option<&str>,
    ) -> ObjectiveArtifact {
        ObjectiveArtifact {
            id: ObjectiveArtifactId::parse(id).unwrap(),
            objective_id: objective_id(),
            plan_id: plan_id.map(|id| ObjectivePlanId::parse(id).unwrap()),
            planner_exchange_id: exchange_id.map(|id| PlannerExchangeId::parse(id).unwrap()),
            kind: "json".to_string(),
            path: format!("{id}.json"),
            sha256: "00".repeat(32),
            byte_len: 128,
            created_at: "2026-01-01T00:00:02Z".to_string(),
        }
    }

    fn sample_message(id: &str, role: &str, artifact_id: Option<&str>) -> NewObjectiveMessage {
        NewObjectiveMessage {
            id: id.parse().unwrap(),
            objective_id: objective_id(),
            role: role.to_string(),
            kind: "preview".to_string(),
            content_objective_artifact_id: artifact_id
                .map(|id| ObjectiveArtifactId::parse(id).unwrap()),
            content_preview: format!("{role} preview"),
            created_at: "2026-01-01T00:00:03Z".to_string(),
        }
    }

    fn sample_event(event_type: &str) -> ObjectiveEvent {
        ObjectiveEvent {
            id: EVENT_1.parse().unwrap(),
            objective_id: objective_id(),
            event_type: event_type.to_string(),
            message: event_type.to_string(),
            payload_json: "{}".to_string(),
            created_at: "2026-01-01T00:00:04Z".to_string(),
        }
    }

    fn objective_id() -> ObjectiveId {
        ObjectiveId::parse(OBJECTIVE_1).unwrap()
    }

    fn count_rows(store: &SqliteTaskStore, table: &str) -> i64 {
        let conn = store.lock_conn().unwrap();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }
}
