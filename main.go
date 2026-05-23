package main

import (
	"context"
	"os"
	"os/signal"

	"github.com/bountyhub-org/runner/cmd"
)

func main() {
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()

	cmd.ExecuteContext(ctx)
}
