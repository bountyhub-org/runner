package expr

import (
	"github.com/bountyhub-org/runner/api/jobexecutionv1connect"
	"github.com/google/cel-go/cel"
)

var celEnv *cel.Env

func init() {
	project := jobexecutionv1connect.Project{}
	e, err := cel.NewEnv(
		cel.Types(
			&project,
			jobexecutionv1connect.Project{},
			jobexecutionv1connect.Workflow{},
			jobexecutionv1connect.Revision{},
		),
		cel.Variable("id", cel.StringType),
		cel.Variable("name", cel.StringType),
		cel.Variable("vars", cel.MapType(cel.StringType, cel.StringType)),
		cel.Variable("secrets", cel.MapType(cel.StringType, cel.StringType)),
		cel.Variable("env", cel.MapType(cel.StringType, cel.StringType)),
		cel.Variable("project", cel.ObjectType("github.com/bountyhub-org/runner/api/jobexecutionv1connect.Project")),
	)
	if err != nil {
		panic(err)
	}
	celEnv = e
}
