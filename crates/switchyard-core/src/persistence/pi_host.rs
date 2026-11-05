use super::*;

impl Store {
    pub(crate) fn pi_host_view(&self, attempt: &str) -> Result<Option<Value>> {
        if user_version(&self.conn)? < 10 {
            return Ok(None);
        }
        let row: Option<(u64, String, String, i64)> = self.conn.query_row(
            "SELECT generation, lifecycle_json, observation_json, observed_at FROM pi_host_sessions WHERE attempt_id=?1",
            [attempt], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).optional().map_err(Error::storage)?;
        row.map(|(generation, lifecycle, observation, at)| {
            Ok(json!({
                "generation":generation,
                "lifecycle":serde_json::from_str::<Value>(&lifecycle).map_err(Error::storage)?,
                "observation":serde_json::from_str::<Value>(&observation).map_err(Error::storage)?,
                "observedAt":at,
            }))
        })
        .transpose()
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    pub(crate) fn save_pi_host(
        &mut self,
        attempt: &str,
        generation: u64,
        lifecycle: &Value,
        observation: &Value,
    ) -> Result<()> {
        if self.generation()? != generation {
            return Err(Error::guard("stale Pi Host observation"));
        }
        let tx = self.immediate()?;
        tx.execute(
            "INSERT INTO pi_host_sessions VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(attempt_id) DO UPDATE SET generation=excluded.generation,
             lifecycle_json=excluded.lifecycle_json, observation_json=excluded.observation_json, observed_at=excluded.observed_at",
            params![attempt, generation, lifecycle.to_string(), observation.to_string(), now_ms()],
        ).map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }
}
