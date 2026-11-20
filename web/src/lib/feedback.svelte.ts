// The notice and the problem a screen shows for what the operator last did.

import { errorMessage } from './api'

export class Feedback {
  notice = $state<string | null>(null)
  problem = $state<string | null>(null)

  /**
   * Runs one request. Shows `done` when it works and the API's message, or `fallback`, when it
   * does not. Returns whether it worked.
   */
  async run(action: () => Promise<void>, done: string, fallback: string): Promise<boolean> {
    this.notice = null
    this.problem = null
    try {
      await action()
      this.notice = done
      return true
    } catch (error) {
      this.problem = errorMessage(error, fallback)
      return false
    }
  }
}
