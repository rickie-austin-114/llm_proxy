<script lang="ts">
  let {
    message,
    onDecision,
  }: {
    message: string;
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
  <h2 id="confirmation-heading">Confirm change</h2>
  <p id="confirmation-message">{message}</p>
  <div class="confirmation-actions">
    <button class="button secondary" onclick={() => onDecision(false)}>Cancel</button>
    <button class="button primary" onclick={() => onDecision(true)}>Continue</button>
  </div>
</dialog>
