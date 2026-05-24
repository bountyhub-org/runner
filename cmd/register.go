package cmd

import (
	"fmt"

	"github.com/spf13/cobra"
)

// registerCmd represents the register command
var registerCmd = &cobra.Command{
	Use:   "register",
	Short: "A brief description of your command",
	PreRunE: func(cmd *cobra.Command, args []string) error {
		return nil
	},
	Run: func(cmd *cobra.Command, args []string) {
		fmt.Println("register called")
	},
}

func init() {
	rootCmd.AddCommand(registerCmd)

	registerCmd.Flags().StringP("token", "t", "", "One-time token for registration")
	registerCmd.Flags().StringP("url", "u", "", "URL of the server to register with")
	registerCmd.Flags().StringP("workdir", "w", "", "Working directory for the agent")
	registerCmd.Flags().Bool("unattended", false, "Run in unattended mode (no interactive prompts)")
	registerCmd.Flags().Bool("replace", false, "Replace existing runner if one is already registered")
}
