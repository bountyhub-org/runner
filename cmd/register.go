package cmd

import (
	"fmt"
	"io/fs"
	"net/url"

	"connectrpc.com/connect"
	"github.com/bountyhub-org/runner/api/runnerregistrationv1connect"
	"github.com/bountyhub-org/runner/config"
	"github.com/bountyhub-org/runner/config/configmanager"
	"github.com/google/uuid"
	"github.com/hashicorp/go-retryablehttp"
	"github.com/spf13/cobra"
)

type registerCmdOptions struct {
	Token      string
	URL        urlFlag
	WorkDir    string
	Name       string
	Unattended bool
	Replace    bool
	Capacity   uint32
}

func (o *registerCmdOptions) Validate() error {
	token, err := uuid.Parse(o.Token)
	if err != nil {
		return fmt.Errorf("invalid token: %w", err)
	}
	if token.Version() != 4 {
		return fmt.Errorf("invalid token: expected a version 4 UUID")
	}
	if o.URL.Scheme != "https" {
		return fmt.Errorf("invalid URL: scheme must be https")
	}
	if o.URL.Host == "" {
		return fmt.Errorf("invalid URL: host is required")
	}
	if o.URL.Path != "" {
		return fmt.Errorf("invalid URL: path must be empty")
	}
	if o.URL.User != nil {
		return fmt.Errorf("invalid URL: user info is not allowed")
	}
	if o.URL.Fragment != "" {
		return fmt.Errorf("invalid URL: fragment is not allowed")
	}

	if !fs.ValidPath(o.WorkDir) {
		return fmt.Errorf("invalid workdir: must be a valid path")
	}

	if o.Capacity > 1024 || o.Capacity == 0 {
		return fmt.Errorf("invalid capacity: must be between 1 and 1024")
	}

	return nil
}

var registerOpts registerCmdOptions

// registerCmd represents the register command
var registerCmd = &cobra.Command{
	Use:   "register",
	Short: "Register creates a new runner registration with the BountyHub server",
	PreRunE: func(cmd *cobra.Command, args []string) error {
		cmd.MarkFlagRequired("token")
		cmd.MarkFlagRequired("url")
		if registerOpts.Unattended {
			cmd.MarkFlagRequired("name")
			if registerOpts.Name == "" {
				cmd.MarkFlagRequired("workdir")
			}
		}

		return registerOpts.Validate()
	},
	RunE: func(cmd *cobra.Command, args []string) error {
		if err := registerOpts.Validate(); err != nil {
			return err
		}

		configManager, err := configmanager.New()
		if err != nil {
			return fmt.Errorf("failed to initialize config manager: %w", err)
		}

		client := runnerregistrationv1connect.NewRunnerRegistrationServiceClient(
			retryablehttp.NewClient().StandardClient(),
			registerOpts.URL.String(),
		)

		if !registerOpts.Replace {
			exists, err := configManager.Exists(registerOpts.Name)
			if err != nil {
				return fmt.Errorf("failed to check for existing runner: %w", err)
			}
			if exists {
				return fmt.Errorf("a runner with the name '%s' is already registered; use --replace to replace it", registerOpts.Name)
			}
		}

		res, err := client.RegisterRunner(cmd.Context(), connect.NewRequest(&runnerregistrationv1connect.RegisterRunnerRequest{
			Token:   registerOpts.Token,
			Name:    registerOpts.Name,
			WorkDir: registerOpts.WorkDir,
			Replace: registerOpts.Replace,
		}))
		if err != nil {
			return fmt.Errorf("failed to register runner: %w", err)
		}

		if registerOpts.Replace {
			if err := configManager.Remove(registerOpts.Name); err != nil {
				return fmt.Errorf("failed to remove existing runner: %w", err)
			}
		}

		cfg := config.Config{
			Token:    uuid.MustParse(res.Msg.Token),
			Name:     registerOpts.Name,
			Capacity: registerOpts.Capacity,
		}
		if err := cfg.Validate(); err != nil {
			return fmt.Errorf("invalid configuration received from server: %w", err)
		}

		return nil
	},
}

func init() {
	rootCmd.AddCommand(registerCmd)
	registerCmd.Flags().StringVarP(&registerOpts.Token, "token", "t", "", "One-time token for registration")
	registerCmd.Flags().VarP(&registerOpts.URL, "url", "u", "URL of the server to register with")
	registerCmd.Flags().StringVarP(&registerOpts.WorkDir, "workdir", "w", "", "Working directory for the agent")
	registerCmd.Flags().BoolVar(&registerOpts.Unattended, "unattended", false, "Run in unattended mode (no interactive prompts)")
	registerCmd.Flags().BoolVar(&registerOpts.Replace, "replace", false, "Replace existing runner if one is already registered")
	registerCmd.Flags().Uint32VarP(&registerOpts.Capacity, "capacity", "c", 1, "Capacity of the runner (number of concurrent jobs it can handle)")
}

type urlFlag struct {
	*url.URL
}

func (f *urlFlag) String() string {
	return f.URL.String()
}

func (f *urlFlag) Set(value string) error {
	u, err := url.Parse(value)
	if err != nil {
		return fmt.Errorf("invalid URL: %w", err)
	}
	f.URL = u
	return nil
}

func (f *urlFlag) Type() string {
	return "url"
}
