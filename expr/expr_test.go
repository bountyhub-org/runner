package expr

import (
	"testing"

	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
	"github.com/google/uuid"
	"github.com/stretchr/testify/require"
)

func uuidv7() string {
	id, err := uuid.NewV7()
	if err != nil {
		panic(err)
	}
	return id.String()
}

func TestEngineEval(t *testing.T) {
	t.Parallel()
	engine := NewEngine(
		&jobexecutionv1connect.ResolveJobResponse{
			Id:   uuidv7(),
			Name: "test-name",
			Project: &jobexecutionv1connect.Project{
				Id: uuidv7(),
			},
			Workflow: &jobexecutionv1connect.Workflow{
				Id: uuidv7(),
			},
			Revision: &jobexecutionv1connect.Revision{
				Id: uuidv7(),
			},
			Vars: map[string]string{
				"var1": "vv1",
				"var2": "vv2",
			},
			Secrets: map[string]string{
				"secret1": "sv1",
			},
			Env: map[string]string{
				"env1": "ev1",
			},
			Inputs: map[string]*jobexecutionv1connect.InputValue{
				"input1": {
					Value: &jobexecutionv1connect.InputValue_String_{
						String_: "iv1",
					},
				},
				"input2": {
					Value: &jobexecutionv1connect.InputValue_Bool{
						Bool: true,
					},
				},
			},
			Steps: []*jobexecutionv1connect.Step{
				{
					Step: &jobexecutionv1connect.Step_Setup{},
				},
				{
					Step: &jobexecutionv1connect.Step_Command{
						Command: &jobexecutionv1connect.CommandStep{
							Run:          "echo 'hello world'",
							Shell:        "bash",
							Cond:         "always",
							AllowFailure: false,
						},
					},
				},
				{
					Step: &jobexecutionv1connect.Step_Teardown{},
				},
			},
			ScanHistories: map[string]*jobexecutionv1connect.ArtifactsContext{
				"scan1": {
					Contexts: map[string]*jobexecutionv1connect.ArtifactContext{
						"context1": {
							HasDiff:     true,
							IsAvailable: true,
						},
						"context2": {
							HasDiff:     false,
							IsAvailable: false,
						},
					},
				},
				"scan2": {
					Contexts: map[string]*jobexecutionv1connect.ArtifactContext{
						"context1": {
							HasDiff:     true,
							IsAvailable: false,
						},
						"context2": {
							HasDiff:     false,
							IsAvailable: true,
						},
					},
				},
			},
		},
	)

	tt := map[string]struct {
		expr string
		val  any
	}{
		"job id": {
			expr: "id",
			val:  engine.data.id,
		},
		"job name": {
			expr: "name",
			val:  engine.data.name,
		},
		"job project id": {
			expr: "project.id",
			val:  engine.data.project.Id,
		},
		"job workflow id": {
			expr: "workflow.id",
			val:  engine.data.workflow.Id,
		},
		"job revision id": {
			expr: "revision.id",
			val:  engine.data.revision.Id,
		},
		"job var": {
			expr: "vars.var1",
			val:  engine.data.vars["var1"],
		},
		"job secret": {
			expr: "secrets.secret1",
			val:  engine.data.secrets["secret1"],
		},
		"job env": {
			expr: "env.env1",
			val:  engine.data.env["env1"],
		},
		"input string": {
			expr: "inputs.input1",
			val:  engine.data.inputs["input1"],
		},
		"job input bool": {
			expr: "inputs.input2",
			val:  engine.data.inputs["input2"],
		},
		"scan 1 has diff": {
			expr: "scans.scan1.context1.hasDiff",
			val:  engine.data.scans["scan1"].Contexts["context1"].HasDiff,
		},
		"scan 1 is available": {
			expr: "scans.scan1.context1.isAvailable",
			val:  engine.data.scans["scan1"].Contexts["context1"].IsAvailable,
		},
		"scan 2 has diff": {
			expr: "scans.scan2.context1.hasDiff",
			val:  engine.data.scans["scan2"].Contexts["context1"].HasDiff,
		},
		"scan 2 is available": {
			expr: `scans.scan2.has_diff("aname.zip")`,
			val:  engine.data.scans["scan2"].Contexts["context1"].IsAvailable,
		},
	}

	for name, tc := range tt {
		t.Run(name, func(t *testing.T) {
			t.Parallel()
			val, err := engine.Eval(tc.expr)
			require.NoError(t, err, "unexpected error evaluating expression")
			require.Equal(t, tc.val, val.Value(), "unexpected value")
		})
	}
}
