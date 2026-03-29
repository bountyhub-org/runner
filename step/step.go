package step

import "fmt"

type Status int

const (
	_ Status = iota
	StatusSucceeded
	StatusFailed
	StatusSkipped
)

func (s Status) Validate() error {
	switch s {
	case StatusSucceeded, StatusFailed, StatusSkipped:
		return nil
	default:
		return fmt.Errorf("invalid step status: %v", s)
	}
}

func (s Status) String() string {
	switch s {
	case StatusSucceeded:
		return "succeeded"
	case StatusFailed:
		return "failed"
	case StatusSkipped:
		return "skipped"
	default:
		return fmt.Sprintf("unknown status %d", s)
	}
}

type Outcome int

const (
	_ Outcome = iota
	OutcomeSucceeded
	OutcomeFailed
	OutcomeCancelled
)

func (o Outcome) Validate() error {
	switch o {
	case OutcomeSucceeded, OutcomeFailed, OutcomeCancelled:
		return nil
	default:
		return fmt.Errorf("invalid step outcome: %v", o)
	}
}

func (o Outcome) String() string {
	switch o {
	case OutcomeSucceeded:
		return "succeeded"
	case OutcomeFailed:
		return "failed"
	case OutcomeCancelled:
		return "cancelled"
	default:
		return fmt.Sprintf("unknown outcome %d", o)
	}
}

type Result struct {
	Status  Status
	Outcome Outcome
}

func (s Result) IsZero() bool {
	return s.Status == 0 || s.Outcome == 0
}

func (s Result) Validate() error {
	if s.IsZero() {
		return fmt.Errorf("step context is not done with status %v and outcome %v", s.Status, s.Outcome)
	}

	if err := s.Status.Validate(); err != nil {
		return fmt.Errorf("invalid step status: %w", err)
	}
	if err := s.Outcome.Validate(); err != nil {
		return fmt.Errorf("invalid step outcome: %w", err)
	}

	if s.Outcome == OutcomeCancelled && s.Status != StatusFailed {
		return fmt.Errorf("step context has invalid state with status %v and outcome %v", s.Status, s.Outcome)
	}
	if s.Outcome == OutcomeFailed && s.Status != StatusFailed {
		return fmt.Errorf("step context has invalid state with status %v and outcome %v", s.Status, s.Outcome)
	}

	return nil
}

func ResultFailed() Result {
	return Result{
		Status:  StatusFailed,
		Outcome: OutcomeFailed,
	}
}

func ResultCancelled() Result {
	return Result{
		Status:  StatusFailed,
		Outcome: OutcomeCancelled,
	}
}

// ResultSucceeded returns a Result with the given status and OutcomeSucceeded.
func ResultSucceeded(status Status) Result {
	return Result{
		Status:  status,
		Outcome: OutcomeSucceeded,
	}
}
