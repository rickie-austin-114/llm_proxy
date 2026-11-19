<script lang="ts">
  let {
    message,
    heading = 'Confirm change',
    confirmLabel = 'Continue',
    cancelLabel = 'Cancel',
    onDecision,
  }: {
    message: string;
    heading?: string;
    confirmLabel?: string;
    cancelLabel?: string;
    onDecision: (confirmed: boolean) => void;
  } = $props();
  let dialog: HTMLDialogElement;

  $effect(() => {
    if (message && !dialog.open) dialog.showModal();
    else if (!message && dialog.open) dialog.close();
  });
</script>

<dialog
  bind:this={dialog}
  aria-labelledby="confirmation-heading"
  aria-describedby="confirmation-message"
  oncancel={(event) => {
    event.preventDefault();
    onDecision(false);
  }}
>
  <h2 id="confirmation-heading">{heading}</h2>
  <p id="confirmation-message">{message}</p>
  <div class="confirmation-actions">
    <button class="button secondary" onclick={() => onDecision(false)}>{cancelLabel}</button>
    <button class="button primary" onclick={() => onDecision(true)}>{confirmLabel}</button>
  </div>
</dialog>
