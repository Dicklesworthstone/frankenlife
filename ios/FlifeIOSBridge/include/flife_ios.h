#ifndef FLIFE_IOS_H
#define FLIFE_IOS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* =========================================================================
 * Native iOS Metal Compute Controller Backend Callback Vtable
 *
 * Bridges Swift's MTLDevice / MTLComputeCommandEncoder directly into
 * `flife_neural::ControllerBackend` for exact 0-ULP bit-for-bit execution
 * of `softfloat64.metal` + `controller.metal` on Apple Silicon iPhone GPUs.
 * ========================================================================= */

typedef struct FlifeMetalShardDispatch {
    uint64_t shard_handle;
    const uint32_t *job_words; /* 5 * job_count u32 words */
    uint32_t job_count;
    uint32_t threads_per_tg;   /* 32 for widest_hidden <= 64, else 64 */
    uint32_t is_f16;           /* 0 = controller_forward, 1 = controller_forward_half */
} FlifeMetalShardDispatch;

typedef struct FlifeMetalVtable {
    void *ctx;
    uint64_t max_buffer_bytes;
    uint64_t recommended_working_set_bytes;
    const char *device_name;

    /* Allocate a shared Metal parameter shard buffer; returns non-zero handle or 0 on error.
     * If `out_contents` is non-NULL, stores the unified-memory CPU pointer (`MTLBuffer.contents()`)
     * for direct lock-free parallel shard packing. */
    uint64_t (*alloc_shard)(void *ctx, uint64_t bytes, uint8_t **out_contents);
    /* Release a previously allocated shard handle. */
    void (*free_shard)(void *ctx, uint64_t handle);
    /* Write `len` bytes into `handle` at `byte_offset`. Returns 0 on success. */
    int32_t (*write_shard)(
        void *ctx,
        uint64_t handle,
        uint64_t byte_offset,
        const uint8_t *data,
        uint64_t len
    );
    /* Copy `byte_len` bytes from `src_handle` to `dst_handle` (shard growth). Returns 0 on success. */
    int32_t (*copy_shard)(
        void *ctx,
        uint64_t src_handle,
        uint64_t dst_handle,
        uint64_t byte_len
    );
    /* Dispatch all active shards in a single concurrent MTLCommandBuffer. Returns 0 on success. */
    int32_t (*dispatch_batch)(
        void *ctx,
        const FlifeMetalShardDispatch *dispatches,
        uint32_t dispatch_count,
        uint32_t total_jobs,
        uint32_t input_width,
        uint32_t output_width,
        const float *inputs_f32,
        float *outputs_f32,
        uint32_t *faults_u32,
        uint64_t *out_gpu_ns
    );
} FlifeMetalVtable;

/* =========================================================================
 * Opaque Session & Leased Epoch Handles
 * ========================================================================= */

typedef struct FlifeSession FlifeSession;
typedef struct FlifeEpochHandle FlifeEpochHandle;

/* =========================================================================
 * Zero-Copy Leased Render Epoch View (Valid until flife_epoch_release)
 * ========================================================================= */

typedef struct FlifeEpochView {
    uint64_t epoch_id;
    uint64_t tick;
    uint64_t resource_epoch;
    double dt_seconds;
    double world_width;
    double world_height;
    double maximum_speed;
    double attempt_energy;
    double affinity_limit;
    uint32_t population_capacity;

    /* Organism views (all length `organism_count`, in strictly increasing ID order). */
    uint32_t organism_count;
    const uint64_t *ids;
    const float *positions;      /* 2 * organism_count: [x, y] */
    const float *velocities;     /* 2 * organism_count: [vx, vy] */
    const float *energies;       /* organism_count */
    const uint64_t *lineages;    /* organism_count */
    const uint32_t *generations; /* organism_count */
    const uint8_t *topologies;   /* organism_count */
    const float *affinities;     /* 3 * organism_count: Hadamard [q0, q1, q2] */
    const int8_t *cell_states;   /* optional V2: NULL if absent, else organism_count */
    const int16_t *polarities;   /* optional V2: NULL if absent, else organism_count */

    /* Bounded field texture views (all length `field_texels_x * field_texels_y`). */
    uint32_t field_cells_x;
    uint32_t field_cells_y;
    uint32_t field_texels_x;
    uint32_t field_texels_y;
    const float *field_energy;           /* texels_x * texels_y */
    const float *field_composition;      /* 3 * texels_x * texels_y */
    const int8_t *field_components;      /* 32 * texels_x * texels_y */
    const float *field_trail_energy;     /* optional V2: NULL if absent */
    const float *field_vascular;         /* optional V2: NULL if absent */

    /* World metrics & INV-03 energy ledger closure. */
    uint64_t live_count;
    double organism_energy_total;
    double field_energy_total;
    uint32_t max_generation;
    uint64_t total_events;
    uint64_t total_parameters;
    uint64_t genome_logical_bytes;
    uint64_t genome_allocated_bytes;

    double ledger_initial;
    double ledger_current;
    double ledger_injected;
    double ledger_removed;
    double ledger_dissipated;
    double ledger_turnover;
    double ledger_residual;
    double ledger_normalized; /* NaN if denominator is 0 */
    bool has_ledger_normalized;
    bool faulted;

    /* Running tally since founding (valid when has_tally == true). */
    bool has_tally;
    uint64_t tally_births;
    uint64_t tally_introductions;
    uint64_t tally_deaths;
    uint64_t tally_evaluations;
    uint64_t tally_rejected_births;
    uint64_t tally_refused_births;
    double tally_uptake;

    /* Followed lineage status. */
    bool has_followed;
    uint64_t followed_origin_id;
    uint64_t followed_organism_id;
    uint64_t followed_handoffs;
    bool followed_alive;

    /* Live honest clock & Metal backend telemetry. */
    double sim_ticks_per_second;
    double last_tick_wall_ms;
    double last_gpu_ms;
    uint64_t metal_batched_evals;
    uint64_t metal_deferred_evals;
    uint64_t metal_resident_peak_bytes;
    uint64_t metal_total_device_ns;
    bool metal_active;
    bool is_playing;
    bool is_capacity_paused;
} FlifeEpochView;

/* =========================================================================
 * Bounded Organism Inspector Snapshot (INV-09 pure read)
 * ========================================================================= */

typedef struct FlifeOrganismInfo {
    bool found;
    uint64_t tick;
    uint64_t id;
    uint64_t genome_id;
    bool has_parent;
    uint64_t parent_id;
    uint64_t lineage_id;
    uint32_t generation;
    uint64_t birth_tick;
    uint64_t age_ticks;

    double pos_x;
    double pos_y;
    double vel_x;
    double vel_y;
    double energy;
    double energy_change_rate;

    uint32_t topology_index;
    uint32_t hidden_layer_count;
    uint32_t hidden_widths[4];
    uint64_t parameter_count;

    double local_density;
    double local_request;
    double local_gain;
    double local_toxicity;

    double affinity[32];

    bool has_secretory_readiness;
    double secretory_readiness;
    bool has_neighbor_h0;
    double neighbor_h0;

    bool has_descendants;
    uint64_t children_born;
    uint64_t children_alive;
    uint64_t descendants_born;
    uint64_t descendants_alive;
} FlifeOrganismInfo;

/* =========================================================================
 * Non-Invasive Region Measurement (INV-09 pure read)
 * ========================================================================= */

typedef struct FlifeRegionMeasure {
    bool valid;
    uint64_t tick;
    double center_x;
    double center_y;
    double radius;
    uint64_t organisms;
    double organism_energy;
    uint64_t lineages;
    uint64_t cells;
    double field_energy;
} FlifeRegionMeasure;

/* =========================================================================
 * Preset Metadata & Preflight Summary
 * ========================================================================= */

typedef struct FlifePresetSummary {
    bool valid;
    char name[68];
    char model[68];
    char color_mapping[68];
    char weight_dtype[16];
    double world_width;
    double world_height;
    uint32_t grid_width;
    uint32_t grid_height;
    uint32_t initial_count;
    uint32_t maximum_count;
    uint32_t tick_numerator;
    uint32_t tick_denominator;
    uint64_t max_logical_genome_bytes;
    uint64_t max_total_bytes;
    uint64_t v2_feature_mask;
    bool has_contact;
    bool has_v2_visuals;
    bool has_trail_carrier;
    bool has_vascular_field;
    bool has_cell_state;
    bool has_polarity;
    char error[256];
} FlifePresetSummary;

/* =========================================================================
 * Bit-for-Bit Metal Parity Verification Result (XCTest + Live Diagnostics)
 * ========================================================================= */

typedef struct FlifeParityResult {
    bool success;
    bool digest_matched;
    uint64_t ticks_verified;
    uint64_t mismatch_tick;
    uint64_t live_final;
    uint64_t metal_batched;
    uint64_t metal_deferred;
    uint64_t metal_device_ns;
    uint64_t metal_wall_ns;
    uint64_t cpu_wall_ns;
    double ledger_residual;
    char metal_digest_hex[65];
    char cpu_digest_hex[65];
    char error[256];
} FlifeParityResult;

/* =========================================================================
 * Pause Status (read without a new epoch: a paused world publishes none)
 * ========================================================================= */

typedef struct FlifePauseStatus {
    bool is_playing;
    bool is_capacity_paused;
    /* The last step was refused: the device had `available_bytes` free and the
     * step needs `needed_bytes` (its births staging and reserve, plan §19.4). */
    bool is_memory_short;
    uint64_t available_bytes;
    uint64_t needed_bytes;
} FlifePauseStatus;

/* =========================================================================
 * Session Lifecycle & Simulation Controls
 * ========================================================================= */

/* Founds the world and admits it under its configuration's memory budget
 * (FL-043, as `flife run` does). Fails, with the reason in `err_buf`, when the
 * budget is exceeded or when the device (os_proc_available_memory) has less
 * free than one step needs. The Metal backend holds at most the admission's
 * backend term and leaves the device a step's memory; a shard that does not
 * fit is evaluated on the reference, which gives the same bits. */
FlifeSession *flife_session_create(
    const char *config_toml,
    const FlifeMetalVtable *metal_opt,
    uint32_t cpu_workers,
    char *err_buf,
    size_t err_cap
);

void flife_session_destroy(FlifeSession *session);

void flife_session_set_playing(FlifeSession *session, bool playing);
/* Pause simulation and wait up to `timeout_ms` for any active GPU tick to finish (iOS background safety). */
bool flife_session_pause_and_quiesce(FlifeSession *session, uint32_t timeout_ms);
void flife_session_step_once(FlifeSession *session);
void flife_session_set_target_tps(FlifeSession *session, double target_tps);
/* A step the device cannot hold is not taken: the world pauses at that
 * unchanged boundary (is_capacity_paused and is_memory_short) until played or
 * stepped again, when the check is made afresh. */
bool flife_session_pause_status(FlifeSession *session, FlifePauseStatus *out_status);
/* For tests: report `bytes` as the memory the device leaves this process, in
 * place of os_proc_available_memory; 0 restores the platform's figure. */
void flife_set_available_memory_override(uint64_t bytes);

/* Zero-copy latest epoch acquisition (returns NULL if no newer epoch than last_seen_epoch_id). */
FlifeEpochHandle *flife_session_acquire_latest_epoch(
    FlifeSession *session,
    uint64_t last_seen_epoch_id,
    FlifeEpochView *out_view
);

void flife_epoch_release(FlifeEpochHandle *handle);

/* Scientific interventions (applied at next committed tick boundary). */
bool flife_session_inject_energy(
    FlifeSession *session,
    uint32_t cell_x,
    uint32_t cell_y,
    double amount
);

bool flife_session_inject_material(
    FlifeSession *session,
    uint32_t cell_x,
    uint32_t cell_y,
    double amount,
    const double *direction32
);

bool flife_session_introduce_organisms(
    FlifeSession *session,
    uint32_t cell_x,
    uint32_t cell_y,
    uint32_t count,
    double energy
);

/* disposition: 0 = Remove, 1 = Dissipate, 2 = Recycle */
bool flife_session_remove_organisms(
    FlifeSession *session,
    double center_x,
    double center_y,
    double radius,
    uint32_t disposition
);

/* Pure read-only inspection, watch, follow, and region measurement (INV-09).
 * `flife_session_inspect_organism` returns `true` when the worker answered the
 * query (inspect `out_info->found` to distinguish alive vs dead/absent) and
 * `false` only on invalid input, shutdown, or query timeout. */
bool flife_session_inspect_organism(
    FlifeSession *session,
    uint64_t organism_id,
    FlifeOrganismInfo *out_info
);

void flife_session_set_watch(FlifeSession *session, bool enabled, uint64_t organism_id);
void flife_session_set_follow(FlifeSession *session, bool enabled, uint64_t organism_id);

bool flife_session_measure_region(
    FlifeSession *session,
    double center_x,
    double center_y,
    double radius,
    FlifeRegionMeasure *out_measure
);

/* Preset parser & bit-for-bit Metal parity oracle verifier. */
void flife_inspect_preset(const char *config_toml, FlifePresetSummary *out_summary);

void flife_verify_metal_parity(
    const char *config_toml,
    const FlifeMetalVtable *metal,
    uint32_t ticks,
    uint32_t cpu_workers,
    FlifeParityResult *out_result
);

#ifdef __cplusplus
}
#endif

#endif /* FLIFE_IOS_H */
