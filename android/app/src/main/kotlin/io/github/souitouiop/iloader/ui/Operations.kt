package io.github.souitouiop.iloader.ui

import androidx.annotation.StringRes
import io.github.souitouiop.iloader.R
import io.github.souitouiop.iloader.core.IloaderException
import io.github.souitouiop.iloader.core.OperationIds

class OperationStep(val id: String, @StringRes val titleRes: Int)

/** Port of `src/components/operations.ts`. */
enum class OperationKind(
    val id: String,
    @StringRes val titleRes: Int,
    @StringRes val successTitleRes: Int?,
    @StringRes val successMessageRes: Int?,
    val steps: List<OperationStep>,
) {
    SideStore(
        OperationIds.INSTALL_SIDESTORE,
        R.string.operations_install_sidestore_title,
        R.string.operations_install_sidestore_success_title,
        R.string.operations_install_sidestore_success_message,
        listOf(
            OperationStep("download", R.string.operations_install_sidestore_step_download),
            OperationStep("install", R.string.operations_install_sidestore_step_install),
            OperationStep("pairing", R.string.operations_install_sidestore_step_pairing),
        ),
    ),
    LiveContainer(
        OperationIds.INSTALL_SIDESTORE,
        R.string.operations_install_livecontainer_title,
        R.string.operations_install_livecontainer_success_title,
        R.string.operations_install_livecontainer_success_message,
        listOf(
            OperationStep("download", R.string.operations_install_livecontainer_step_download),
            OperationStep("install", R.string.operations_install_livecontainer_step_install),
            OperationStep("pairing", R.string.operations_install_livecontainer_step_pairing),
        ),
    ),
    Sideload(
        OperationIds.SIDELOAD,
        R.string.operations_sideload_title,
        null,
        null,
        listOf(OperationStep("install", R.string.operations_sideload_step_install)),
    ),
}

class StepFailure(val stepId: String, val error: IloaderException)

data class OperationState(
    val kind: OperationKind,
    val started: List<String> = emptyList(),
    val completed: List<String> = emptyList(),
    val failed: List<StepFailure> = emptyList(),
) {
    val hasFailed get() = failed.isNotEmpty()

    /** Same rule as `OperationView.tsx`. */
    val done: Boolean
        get() = (hasFailed && started.size == completed.size + failed.size) ||
            completed.size == kind.steps.size
}
